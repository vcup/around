//! UDP transport — datagram-based IPC.
//!
//! Each UDP datagram carries one complete IPC command.
//! Responses are sent back to the sender's address.
//! Datagrams exceeding 65507 bytes are rejected.

use std::sync::Arc;

use tokio::io::BufReader;
use tokio::net::UdpSocket;

use crate::ipc::codec::IpcCodec;
use crate::Engine;
/// Max UDP payload: 65507 = 65535 (IP) - 8 (UDP header) - 20 (IP header)
const MAX_DATAGRAM: usize = 65507;
/// Soft limit — datagrams above this trigger a warning (non-fragmented delivery not guaranteed)
const WARN_DATAGRAM: usize = 512;

/// A single-datagram reader for use with `IpcCodec::read_command`.
struct DatagramReader {
  data: Vec<u8>,
  pos: usize,
}

impl DatagramReader {
  fn new(data: Vec<u8>) -> Self {
    Self { data, pos: 0 }
  }
}

impl tokio::io::AsyncRead for DatagramReader {
  fn poll_read(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
    buf: &mut tokio::io::ReadBuf<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    let this = self.get_mut();
    let remaining = this.data.len() - this.pos;
    let to_read = std::cmp::min(remaining, buf.remaining());
    buf.put_slice(&this.data[this.pos..this.pos + to_read]);
    this.pos += to_read;
    std::task::Poll::Ready(Ok(()))
  }
}

/// A growable `Vec<u8>` writer for use with `IpcCodec::write_response`.
struct VecWriter<'a>(&'a mut Vec<u8>);

impl<'a> tokio::io::AsyncWrite for VecWriter<'a> {
  fn poll_write(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
    buf: &[u8],
  ) -> std::task::Poll<Result<usize, std::io::Error>> {
    self.get_mut().0.extend_from_slice(buf);
    std::task::Poll::Ready(Ok(buf.len()))
  }

  fn poll_flush(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Result<(), std::io::Error>> {
    std::task::Poll::Ready(Ok(()))
  }

  fn poll_shutdown(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Result<(), std::io::Error>> {
    std::task::Poll::Ready(Ok(()))
  }
}

/// Run a UDP IPC listener. Each datagram is one complete command.
/// Responses are sent to the sender's address.
pub(crate) async fn serve_udp<C: IpcCodec + Clone>(
  engine: Arc<Engine>,
  socket: Arc<UdpSocket>,
  codec: C,
) {
  let mut buf = vec![0u8; MAX_DATAGRAM];

  loop {
    if engine.is_shutdown() {
      break;
    }

    let (n, sender) = match socket.recv_from(&mut buf).await {
      Ok(result) => result,
      Err(e) => {
        tracing::warn!(?e, "UDP recv error");
        continue;
      }
    };

    if n > MAX_DATAGRAM {
      tracing::warn!(n, "dropping oversized UDP datagram");
      continue;
    }
    if n > WARN_DATAGRAM {
      tracing::warn!(n, "large UDP datagram — may fragment");
    }

    let data = buf[..n].to_vec();
    let reader = DatagramReader::new(data);
    let mut reader = BufReader::new(reader);

    let cmd = match codec.read_command(&mut reader).await {
      Ok(c) => c,
      Err(e) => {
        tracing::warn!(?e, "failed to parse UDP command");
        continue;
      }
    };

    let resp = crate::ipc::handle_command(cmd, &engine).await;

    let mut response_buf = Vec::new();
    {
      let mut writer = VecWriter(&mut response_buf);
      if let Err(e) = codec.write_response(&mut writer, &resp).await {
        tracing::warn!(?e, "failed to encode UDP response");
        continue;
      }
    }

    if let Err(e) = socket.send_to(&response_buf, sender).await {
      tracing::warn!(?e, "failed to send UDP response");
    }
  }

  tracing::info!("UDP listener shutting down");
}
