//! Connection handling: accept loop + per-command command processing.

use super::handle_command;
use crate::ipc::codec::{IpcCodec, IpcWire};
use crate::ipc::types::{IpcCommand, PlaybackState};
use crate::pipeline::Engine;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

/// Accept loop — processes connections sequentially.
pub(crate) async fn serve<F, S>(
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  codec: IpcWire,
  mut accept: F,
) where
  F: FnMut() -> Pin<Box<dyn Future<Output = io::Result<S>> + Send>>,
  S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
  loop {
    if engine.is_shutdown() {
      break;
    }

    let stream = match accept().await {
      Ok(s) => s,
      Err(e) => {
        tracing::warn!(?e, "accept error");
        continue;
      }
    };

    let eng = engine.clone();
    let st = state.clone();
    let cdc = codec.clone();

    if let Err(e) = handle_connection(stream, eng, st, cdc).await {
      tracing::error!(?e, "connection error");
    }
  }
}

/// Process commands from a single client connection until EOF or error.
pub(crate) async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
  stream: S,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
  codec: impl IpcCodec,
) -> io::Result<()> {
  let (reader, mut writer) = tokio::io::split(stream);
  let mut reader = BufReader::new(reader);

  loop {
    let cmd: IpcCommand = match codec.read_command(&mut reader).await {
      Ok(c) => c,
      Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
      Err(e) => {
        tracing::warn!(?e, "failed to read IPC command");
        break;
      }
    };

    let resp = handle_command(cmd, &engine, &state).await;
    if let Err(e) = codec.write_response(&mut writer, &resp).await {
      tracing::warn!(?e, "failed to write IPC response");
      break;
    }
  }

  Ok(())
}
