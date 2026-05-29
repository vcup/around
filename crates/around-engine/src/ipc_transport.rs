//! Platform-neutral IPC transport: Unix domain sockets, TCP localhost.
//!
//! Uses an enum-based approach — zero allocation, no trait objects.
//! Codec (serialisation format) is independent of transport (byte I/O).

use std::io::{self, Read, Write};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};

// ---------------------------------------------------------------------------
// IpcAddr
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum IpcAddr {
  #[cfg(unix)]
  UnixSocket(std::path::PathBuf),
  Tcp {
    host: String,
    port: u16,
  },
}

impl IpcAddr {
  pub fn default_local() -> Self {
    #[cfg(unix)]
    {
      Self::UnixSocket(std::path::PathBuf::from("/tmp/around.sock"))
    }
    #[cfg(not(unix))]
    {
      Self::Tcp {
        host: "127.0.0.1".into(),
        port: 0,
      }
    }
  }

  pub fn display(&self) -> String {
    match self {
      #[cfg(unix)]
      Self::UnixSocket(p) => p.display().to_string(),
      Self::Tcp { host, port } => format!("{}:{}", host, port),
    }
  }

  /// Check whether another instance is already bound.
  pub(crate) async fn is_bound(&self) -> bool {
    match self {
      #[cfg(unix)]
      Self::UnixSocket(path) => tokio::net::UnixStream::connect(path).await.is_ok(),
      Self::Tcp { host, port } => TcpStream::connect(format!("{}:{}", host, port))
        .await
        .is_ok(),
    }
  }

  /// Clean up stale resources (socket files).
  pub(crate) fn cleanup(&self) {
    #[cfg(unix)]
    if let Self::UnixSocket(path) = self {
      let _ = std::fs::remove_file(path);
    }
  }
}

// ---------------------------------------------------------------------------
// IpcListener
// ---------------------------------------------------------------------------

pub enum IpcListener {
  #[cfg(unix)]
  Unix(tokio::net::UnixListener),
  Tcp(TokioTcpListener),
}

impl IpcListener {
  pub async fn bind(addr: &IpcAddr) -> io::Result<Self> {
    match addr {
      #[cfg(unix)]
      IpcAddr::UnixSocket(path) => {
        let _ = std::fs::remove_file(path); // remove stale socket
        let inner = tokio::net::UnixListener::bind(path)?;
        // Set permissions to 0600.
        if let Ok(meta) = std::fs::metadata(path) {
          use std::os::unix::fs::PermissionsExt;
          let mut perms = meta.permissions();
          perms.set_mode(0o600);
          let _ = std::fs::set_permissions(path, perms);
        }
        Ok(Self::Unix(inner))
      }
      IpcAddr::Tcp { host, port } => {
        let inner = TokioTcpListener::bind(format!("{}:{}", host, port)).await?;
        Ok(Self::Tcp(inner))
      }
    }
  }

  pub(crate) async fn accept(&self) -> io::Result<IpcStream> {
    match self {
      #[cfg(unix)]
      Self::Unix(l) => {
        let (stream, _) = l.accept().await?;
        Ok(IpcStream::Unix(stream))
      }
      Self::Tcp(l) => {
        let (stream, _) = l.accept().await?;
        Ok(IpcStream::Tcp(stream))
      }
    }
  }

  pub(crate) fn local_addr(&self) -> io::Result<IpcAddr> {
    match self {
      #[cfg(unix)]
      Self::Unix(l) => {
        let path = l
          .local_addr()?
          .as_pathname()
          .map(|p| p.to_path_buf())
          .unwrap_or_default();
        Ok(IpcAddr::UnixSocket(path))
      }
      Self::Tcp(l) => {
        let addr = l.local_addr()?;
        Ok(IpcAddr::Tcp {
          host: "127.0.0.1".into(),
          port: addr.port(),
        })
      }
    }
  }
}

// ---------------------------------------------------------------------------
// IpcStream
// ---------------------------------------------------------------------------

pub enum IpcStream {
  #[cfg(unix)]
  Unix(tokio::net::UnixStream),
  Tcp(TcpStream),
}

impl AsyncRead for IpcStream {
  fn poll_read(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
    buf: &mut tokio::io::ReadBuf<'_>,
  ) -> std::task::Poll<io::Result<()>> {
    match &mut *self {
      #[cfg(unix)]
      Self::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
      Self::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
    }
  }
}

impl AsyncWrite for IpcStream {
  fn poll_write(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
    buf: &[u8],
  ) -> std::task::Poll<io::Result<usize>> {
    match &mut *self {
      #[cfg(unix)]
      Self::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
      Self::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
    }
  }

  fn poll_flush(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<io::Result<()>> {
    match &mut *self {
      #[cfg(unix)]
      Self::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
      Self::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
    }
  }

  fn poll_shutdown(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<io::Result<()>> {
    match &mut *self {
      #[cfg(unix)]
      Self::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
      Self::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
    }
  }
}

// ---------------------------------------------------------------------------
// Sync client connect (CLI, tests)
// ---------------------------------------------------------------------------

/// Synchronous bidirectional stream for IPC clients.
pub enum IpcSyncStream {
  #[cfg(unix)]
  Unix(std::os::unix::net::UnixStream),
  Tcp(std::net::TcpStream),
}

impl Read for IpcSyncStream {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    match self {
      #[cfg(unix)]
      Self::Unix(s) => s.read(buf),
      Self::Tcp(s) => s.read(buf),
    }
  }
}

impl Write for IpcSyncStream {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    match self {
      #[cfg(unix)]
      Self::Unix(s) => s.write(buf),
      Self::Tcp(s) => s.write(buf),
    }
  }
  fn flush(&mut self) -> io::Result<()> {
    match self {
      #[cfg(unix)]
      Self::Unix(s) => s.flush(),
      Self::Tcp(s) => s.flush(),
    }
  }
}
/// Connect to an IPC address synchronously.
pub fn connect_sync(addr: &IpcAddr) -> io::Result<IpcSyncStream> {
  match addr {
    #[cfg(unix)]
    IpcAddr::UnixSocket(path) => {
      let stream = std::os::unix::net::UnixStream::connect(path)?;
      Ok(IpcSyncStream::Unix(stream))
    }
    IpcAddr::Tcp { host, port } => {
      let stream = std::net::TcpStream::connect(format!("{}:{}", host, port))?;
      Ok(IpcSyncStream::Tcp(stream))
    }
  }
}

/// Connect to an IPC address asynchronously.
pub async fn connect(addr: &IpcAddr) -> io::Result<IpcStream> {
  match addr {
    #[cfg(unix)]
    IpcAddr::UnixSocket(path) => {
      let stream = tokio::net::UnixStream::connect(path).await?;
      Ok(IpcStream::Unix(stream))
    }
    IpcAddr::Tcp { host, port } => {
      let stream = TcpStream::connect(format!("{}:{}", host, port)).await?;
      Ok(IpcStream::Tcp(stream))
    }
  }
}
