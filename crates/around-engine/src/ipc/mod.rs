//! IPC server: multi-transport with platform-native fallback.
//!
//! Architecture: transports (unix/win/tcp/udp) → codec → handlers.
//! All transports run concurrently; codec is transport-agnostic.

pub(crate) mod codec;
#[cfg(feature = "protobuf-ipc")]
pub(crate) mod codec_proto;
pub(crate) mod config;
pub(crate) mod connection;
pub(crate) mod handlers;
pub mod transport_manager;
pub(crate) mod transport_udp;
#[cfg(unix)]
pub mod transport_unix;
#[cfg(windows)]
pub(crate) mod transport_win;

// Safety net: refuse to compile without any IPC codec.
#[cfg(not(any(feature = "protobuf-ipc", feature = "json-ipc")))]
compile_error!("at least one IPC codec feature must be enabled (protobuf-ipc or json-ipc)");

/// Protobuf-generated types — codec implementation pending (Phase 10).
#[cfg(feature = "protobuf-ipc")]
mod proto {
  include!(concat!(env!("OUT_DIR"), "/around.ipc.rs"));
}
pub mod types;

pub use self::config::IpcConfig;

use self::types::{IpcCommand, IpcResponse};
use crate::pipeline::Engine;
use std::sync::Arc;
// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub(crate) async fn handle_command(cmd: IpcCommand, engine: &Arc<Engine>) -> IpcResponse {
  use self::handlers::*;
  match cmd {
    IpcCommand::Play { path, stream_id } => handle_play(engine, path, stream_id),
    IpcCommand::Pause { stream_id } => handle_pause(engine, stream_id),
    IpcCommand::Resume { stream_id } => handle_resume(engine, stream_id),
    IpcCommand::Seek {
      position_ms,
      stream_id,
    } => handle_seek(engine, position_ms, stream_id),
    IpcCommand::Stop { stream_id } => handle_stop(engine, stream_id),
    IpcCommand::Status { stream_id } => handle_status(engine, stream_id),
    IpcCommand::ListCodecs => handle_list_codecs(),
    IpcCommand::LoadCodec { path } => handle_load_codec(&path),
    IpcCommand::LoadCodecBytes { data } => handle_load_codec_bytes(data),
    IpcCommand::Cleanup => handle_cleanup(),
    IpcCommand::Shutdown => handle_shutdown(engine),
  }
}

// ---------------------------------------------------------------------------

/// Start the IPC server with the given configuration.
pub async fn run_ipc_server(
  config: IpcConfig,
  engine: Arc<Engine>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Remove stale temp files from crashed previous instances before binding.
  for path in handlers::scan_and_remove_stale_temp_files() {
    tracing::info!("removed stale temp file: {}", path);
  }
  let mut manager = transport_manager::TransportManager::new(config);
  manager.check_running_instance().await?;
  manager.start(engine).await?;
  manager.join().await;
  Ok(())
}
