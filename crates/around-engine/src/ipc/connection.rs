//! Connection handling: accept loop + per-command command processing.

use super::handle_command;
use crate::ipc::codec::{IpcCodec, IpcWire};
use crate::ipc::types::IpcCommand;
use crate::pipeline::Engine;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

/// Accept loop — processes connections sequentially.
pub(crate) async fn serve<F, S>(engine: &Arc<Engine>, codec: IpcWire, mut accept: F)
where
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
    let cdc = codec.clone();

    if let Err(e) = handle_connection(stream, eng, cdc).await {
      tracing::error!(?e, "connection error");
    }
  }
}

/// Process commands from a single client connection until EOF or error.
///
/// The first non-whitespace byte is inspected for JSON (`{`). Any other
/// prefix uses the listener's configured wire format, which is Protobuf by
/// default when that feature is enabled.
pub(crate) async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
  stream: S,
  engine: Arc<Engine>,
  codec: IpcWire,
) -> io::Result<()> {
  let (reader, mut writer) = tokio::io::split(stream);
  let mut reader = BufReader::new(reader);

  // Peek without consuming so the selected codec receives the full frame.
  let codec = {
    let buf = reader.fill_buf().await?;
    if buf.is_empty() {
      return Ok(());
    }
    buf
      .iter()
      .copied()
      .find(|byte| !byte.is_ascii_whitespace())
      .and_then(IpcWire::detect_from_byte)
      .unwrap_or(codec)
  };

  loop {
    let cmd: IpcCommand = match codec.read_command(&mut reader).await {
      Ok(c) => c,
      Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
      Err(e) => {
        tracing::warn!(?e, "failed to read IPC command");
        break;
      }
    };

    let resp = handle_command(cmd, &engine).await;
    if let Err(e) = codec.write_response(&mut writer, &resp).await {
      tracing::warn!(?e, "failed to write IPC response");
      break;
    }
  }

  Ok(())
}

#[cfg(all(test, feature = "protobuf-ipc", feature = "json-ipc"))]
mod tests {
  use super::*;
  use crate::config::OutputDriver;
  use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

  #[tokio::test]
  async fn default_protobuf_listener_accepts_json_line_client() {
    let mut config = crate::EngineConfig::load();
    config.output_driver = OutputDriver::Null;
    let engine = Arc::new(Engine::new(config));
    let (client, server) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(handle_connection(
      server,
      Arc::clone(&engine),
      IpcWire::select(false),
    ));
    let (reader, mut writer) = tokio::io::split(client);
    let mut reader = BufReader::new(reader);

    writer
      .write_all(b"{\"command\":\"status\"}\n")
      .await
      .expect("duplex write must succeed");
    writer.flush().await.expect("duplex flush must succeed");
    let mut response = String::new();
    reader
      .read_line(&mut response)
      .await
      .expect("JSON response line must be readable");

    let value: serde_json::Value =
      serde_json::from_str(response.trim_end()).expect("response must be JSON");
    assert_eq!(value["status"], "ok");

    drop(writer);
    drop(reader);
    server_task
      .await
      .expect("server task must not panic")
      .expect("connection must close cleanly");
  }
}
