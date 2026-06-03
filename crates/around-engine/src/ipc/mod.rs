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
pub(crate) mod transport_manager;
pub(crate) mod transport_udp;
#[cfg(unix)]
pub(crate) mod transport_unix;
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

use self::types::{IpcCommand, IpcResponse, PlaybackState};
use crate::extensions::ExtensionManager;
use crate::pipeline::Engine;
use std::sync::{Arc, Mutex};
// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub(crate) async fn handle_command(
  cmd: IpcCommand,
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  ext_mgr: &Arc<ExtensionManager>,
) -> IpcResponse {
  use self::handlers::*;
  match cmd {
    IpcCommand::Play { path } => handle_play(engine, state, path),
    IpcCommand::Pause => handle_pause(engine, state),
    IpcCommand::Resume => handle_resume(engine, state),
    IpcCommand::Seek { position_ms } => handle_seek(engine, state, position_ms),
    IpcCommand::Stop => handle_stop(engine, state),
    IpcCommand::Status => handle_status(state),
    IpcCommand::ListCodecs => handle_list_codecs(ext_mgr),
    IpcCommand::LoadCodec { path } => handle_load_codec(ext_mgr, &path),
    IpcCommand::LoadCodecBytes { data } => handle_load_codec_bytes(ext_mgr, data),
    IpcCommand::Cleanup => handle_cleanup(ext_mgr),
    IpcCommand::Shutdown => handle_shutdown(engine),
  }
}

// ---------------------------------------------------------------------------

/// Start the IPC server with the given configuration.
pub async fn run_ipc_server(
  config: IpcConfig,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let ext_mgr = Arc::new(ExtensionManager::new());
  // Remove stale temp files from crashed previous instances before binding.
  for path in handlers::scan_and_remove_stale_temp_files() {
    tracing::info!("removed stale temp file: {}", path);
  }
  let mut manager = transport_manager::TransportManager::new(config);
  manager.check_running_instance().await?;
  manager.start(engine, state, ext_mgr).await?;
  manager.join().await;
  Ok(())
}
