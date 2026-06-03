//! IPC server configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

/// IPC server configuration.
///
/// All remote transports are opt-in (disabled by default) per Constitution VI.
/// Platform-native IPC runs by default.
#[derive(Debug, Clone)]
pub struct IpcConfig {
  /// Remote TCP listener address. `None` → no remote TCP.
  pub tcp_bind: Option<SocketAddr>,
  /// Remote UDP listener address. `None` → no remote UDP.
  pub udp_bind: Option<SocketAddr>,
  /// Whether platform-native IPC (Unix socket / named pipe) is enabled. Default true.
  pub enable_platform_native: bool,
  /// Custom port file path for TCP fallback discovery. `None` → use <temp>/around.port.
  pub port_file_path: Option<PathBuf>,
  /// Check for a running engine instance before binding. Set false for tests.
  pub check_running_instance: bool,
  /// Custom Unix socket path. `None` → resolve via platform conventions.
  pub unix_socket_path: Option<PathBuf>,

  /// Force JSON wire format even when protobuf is compiled. For tests.
  pub force_json: bool,
}

impl Default for IpcConfig {
  fn default() -> Self {
    Self {
      tcp_bind: None,
      udp_bind: None,
      enable_platform_native: true,
      port_file_path: None,
      check_running_instance: true,
      unix_socket_path: None,
      force_json: false,
    }
  }
}

impl IpcConfig {
  /// Build IpcConfig from explicit parts (injectable, test-friendly).
  ///
  /// Each pair is `(cli_value, env_value)` with CLI taking precedence.
  /// Pass `None` for absent values. Tests use this to avoid `std::env` mutation.
  pub fn from_parts(
    tcp_cli: Option<&str>,
    tcp_env: Option<&str>,
    udp_cli: Option<&str>,
    udp_env: Option<&str>,
  ) -> Result<Self, Box<dyn std::error::Error>> {
    use std::str::FromStr;

    let resolve = |cli: Option<&str>,
                   env: Option<&str>|
     -> Result<Option<SocketAddr>, Box<dyn std::error::Error>> {
      if let Some(addr) = cli {
        Ok(Some(SocketAddr::from_str(addr)?))
      } else if let Some(addr) = env {
        Ok(Some(SocketAddr::from_str(addr)?))
      } else {
        Ok(None)
      }
    };

    let tcp_bind = resolve(tcp_cli, tcp_env)?;
    let udp_bind = resolve(udp_cli, udp_env)?;

    Ok(Self {
      tcp_bind,
      udp_bind,
      ..Self::default()
    })
  }

  /// Build IpcConfig from CLI flags and environment variables.
  ///
  /// CLI flags take precedence over environment variables.
  /// Environment: `AROUND_IPC_TCP_BIND`, `AROUND_IPC_UDP_BIND` (FR-037).
  pub fn from_cli_env(
    tcp: Option<String>,
    udp: Option<String>,
  ) -> Result<Self, Box<dyn std::error::Error>> {
    Self::from_parts(
      tcp.as_deref(),
      std::env::var("AROUND_IPC_TCP_BIND").ok().as_deref(),
      udp.as_deref(),
      std::env::var("AROUND_IPC_UDP_BIND").ok().as_deref(),
    )
  }
}
