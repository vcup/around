//! Windows named pipe transport using `tokio::net::windows::named_pipe`.
//!
//! Fully async — no `spawn_blocking` or raw Win32 FFI needed.
//! tokio's `net` feature (already enabled) includes named pipe support on Windows.

use crate::ipc::codec::IpcCodec;
use crate::ipc::types::PlaybackState;
use crate::pipeline::Engine;
use std::sync::{Arc, Mutex};

/// Create a named pipe server at `\\.\pipe\around`.
pub(crate) async fn create_named_pipe(
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
  use tokio::net::windows::named_pipe::ServerOptions;

  let pipe_name = r"\\.\pipe\around";
  ServerOptions::new()
    .reject_remote_clients(true)
    .create(pipe_name)
}

/// Accept loop for named pipe clients. Runs one connection at a time per pipe instance.
pub(crate) async fn serve_pipe(
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
  mut server: tokio::net::windows::named_pipe::NamedPipeServer,
  codec: impl IpcCodec + Clone + Send + 'static,
) {
  loop {
    if engine.is_shutdown() {
      break;
    }

    if let Err(e) = server.connect().await {
      tracing::warn!(?e, "named pipe connect error");
      continue;
    }

    let eng = engine.clone();
    let st = state.clone();
    let cdc = codec.clone();

    let next_pipe = match create_named_pipe().await {
      Ok(p) => p,
      Err(e) => {
        tracing::error!(?e, "failed to create next named pipe instance");
        break;
      }
    };

    if let Err(e) = crate::ipc::connection::handle_connection(server, eng, st, cdc).await {
      tracing::error!(?e, "named pipe connection error");
    }
    server = next_pipe;
  }
}
