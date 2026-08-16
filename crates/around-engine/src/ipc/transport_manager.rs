//! Transport orchestration: start-up sequence, lifecycle, shutdown.

use crate::ipc::codec::IpcWire;
use crate::ipc::config::IpcConfig;
use crate::pipeline::Engine;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::{TcpListener, UdpSocket};
use tokio::task::JoinHandle;

/// Manages the lifecycle of all IPC transports.
///
/// Owns the transport configuration and orchestrates:
/// 1. Running-instance detection
/// 2. Transport binding and spawn
/// 3. Graceful join on shutdown
pub(crate) struct TransportManager {
  config: IpcConfig,
  port_path: PathBuf,
  #[cfg(unix)]
  unix_socket: PathBuf,
  handles: Vec<JoinHandle<()>>,
}

impl TransportManager {
  /// Create a new manager from config.
  ///
  /// Resolves the port file path and (on Unix) the socket path eagerly
  /// so they are available for the running-instance check.
  pub fn new(config: IpcConfig) -> Self {
    let port_path = config
      .port_file_path
      .clone()
      .unwrap_or_else(|| std::env::temp_dir().join("around.port"));
    #[cfg(unix)]
    let unix_socket = {
      let (sp, _) = crate::ipc::transport_unix::resolve_socket_dir();
      sp
    };

    Self {
      config,
      port_path,
      #[cfg(unix)]
      unix_socket,
      handles: Vec::new(),
    }
  }

  /// Probe all configured transports for a running engine instance.
  ///
  /// Returns `Ok(())` if no instance is detected, or an error with a
  /// human-readable message.
  pub async fn check_running_instance(
    &self,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !self.config.check_running_instance {
      return Ok(());
    }

    #[cfg(unix)]
    {
      use tokio::net::UnixStream;
      if UnixStream::connect(&self.unix_socket).await.is_ok() {
        return Err("another engine instance is already running (unix socket)".into());
      }
    }

    // TCP probe — try to read port file and connect.
    if let Ok(port_str) = std::fs::read_to_string(&self.port_path) {
      if let Ok(port) = port_str.trim().parse::<u16>() {
        if TcpListener::bind(format!("127.0.0.1:{port}"))
          .await
          .is_err()
        {
          return Err(
            "another engine instance is already running (port file exists and port is in use)"
              .into(),
          );
        }
        // Port file exists but port is free — stale port file from a crash.
        // We'll overwrite it when we bind.
        let _ = std::fs::remove_file(&self.port_path);
      }
    }

    Ok(())
  }

  /// Start all configured transports.
  ///
  /// On Unix: binds a Unix domain socket; falls back to TCP if unavailable.
  /// On Windows: creates a named pipe; falls back to TCP if unavailable.
  /// TCP is always started if native transport fails or an explicit bind
  /// address is configured.  UDP is started when `udp_bind` is set.
  ///
  /// Each transport spawns a tokio task; handles are stored for later join.
  pub async fn start(
    &mut self,
    engine: Arc<Engine>,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut native_ok = false;

    // --- Platform-native transport ---
    if self.config.enable_platform_native {
      #[cfg(unix)]
      {
        let bind_result =
          crate::ipc::transport_unix::bind_unix_socket(self.config.unix_socket_path.as_deref())
            .await;
        if let Ok(listener) = bind_result {
          tracing::info!(
            "IPC server listening on Unix socket at {}",
            self.unix_socket.display()
          );
          native_ok = true;
          let eng = engine.clone();
          let sp = self.unix_socket.clone();
          let force_json = self.config.force_json;
          let handle = tokio::spawn(async move {
            let listener = Arc::new(listener);
            let accept = move || {
              let l = listener.clone();
              Box::pin(async move { l.accept().await.map(|(s, _)| s) })
                as Pin<Box<dyn Future<Output = io::Result<_>> + Send>>
            };
            super::connection::serve(&eng, IpcWire::select(force_json), accept).await;
            // FR-005: clean up socket file on exit.
            let _ = std::fs::remove_file(&sp);
          });
          self.handles.push(handle);
        } else {
          tracing::info!("Unix socket unavailable — using TCP fallback");
        }
      }

      #[cfg(windows)]
      {
        match crate::ipc::transport_win::create_named_pipe().await {
          Ok(server) => {
            tracing::info!("IPC server listening on named pipe");
            native_ok = true;
            let eng = engine.clone();
            let force_json = self.config.force_json;
            let handle = tokio::spawn(async move {
              crate::ipc::transport_win::serve_pipe(eng, server, IpcWire::select(force_json)).await;
            });
            self.handles.push(handle);
          }
          Err(_) => {
            tracing::info!("Named pipe unavailable — will try TCP fallback");
          }
        }
      }
    } // self.config.enable_platform_native

    // --- TCP (fallback when native failed, or explicit bind when configured) ---
    let should_bind_tcp = !native_ok || self.config.tcp_bind.is_some();
    if should_bind_tcp {
      let bind_addr = self
        .config
        .tcp_bind
        .unwrap_or(std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
      let needs_port_file = bind_addr.ip().is_loopback() && bind_addr.port() == 0;

      let tcp_listener = TcpListener::bind(bind_addr).await?;
      let tcp_port = tcp_listener.local_addr()?.port();
      if needs_port_file {
        std::fs::write(&self.port_path, tcp_port.to_string())?;
        #[cfg(unix)]
        {
          use std::os::unix::fs::PermissionsExt;
          let _ = std::fs::set_permissions(&self.port_path, std::fs::Permissions::from_mode(0o600));
        }
      }
      tracing::info!(
        "IPC server (TCP) listening on {}",
        tcp_listener.local_addr()?
      );
      let eng = engine.clone();
      let pp = self.port_path.clone();
      let force_json = self.config.force_json;
      let handle = tokio::spawn(async move {
        let tcp_listener = Arc::new(tcp_listener);
        let accept = move || {
          let l = tcp_listener.clone();
          Box::pin(async move { l.accept().await.map(|(s, _)| s) })
            as Pin<Box<dyn Future<Output = io::Result<_>> + Send>>
        };
        super::connection::serve(&eng, IpcWire::select(force_json), accept).await;
        if needs_port_file {
          let _ = std::fs::remove_file(&pp);
        }
      });
      self.handles.push(handle);
    } else if native_ok {
      // Native transport succeeded — clean up any stale TCP port file.
      let _ = std::fs::remove_file(&self.port_path);
    }

    // --- UDP (always separate, opt-in) ---
    if let Some(addr) = self.config.udp_bind {
      let udp_socket = UdpSocket::bind(addr).await?;
      tracing::info!("IPC server (UDP) listening on {}", udp_socket.local_addr()?);
      let eng = engine.clone();
      let force_json = self.config.force_json;
      let handle = tokio::spawn(async move {
        crate::ipc::transport_udp::serve_udp(
          eng,
          Arc::new(udp_socket),
          IpcWire::select(force_json),
        )
        .await;
      });
      self.handles.push(handle);
    }

    Ok(())
  }

  /// Wait for all spawned transport tasks to complete.
  ///
  /// Transports exit when the engine signals shutdown (via
  /// `Engine::shutdown`), at which point their accept loops break and
  /// any per-transport cleanup (socket / port file removal) runs.
  pub async fn join(self) {
    for h in self.handles {
      let _ = h.await;
    }
  }
}
