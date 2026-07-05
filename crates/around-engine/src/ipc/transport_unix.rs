//! Unix domain socket transport (Linux/macOS).

use std::path::PathBuf;
use tokio::net::UnixListener;

/// Resolve the best available socket path.
pub fn resolve_socket_dir() -> (PathBuf, Option<PathBuf>) {
  if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
    let parent = PathBuf::from(&dir).join("around");
    return (parent.join("around.sock"), Some(parent));
  }
  if let Ok(dir) = std::env::var("TMPDIR") {
    return (PathBuf::from(&dir).join("around.sock"), None);
  }
  (std::env::temp_dir().join("around.sock"), None)
}

/// Bind a Unix socket. When `custom_path` is `Some`, binds at that exact path;
/// otherwise resolves the best platform-appropriate path via `resolve_socket_dir`.
pub(crate) async fn bind_unix_socket(
  custom_path: Option<&std::path::Path>,
) -> std::io::Result<UnixListener> {
  let socket_path;

  if let Some(path) = custom_path {
    socket_path = path.to_path_buf();
  } else {
    let (sp, parent_dir) = resolve_socket_dir();
    socket_path = sp;
    if let Some(dir) = &parent_dir {
      if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
          use std::os::unix::fs::PermissionsExt;
          std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
      }
    }
  }

  let _ = std::fs::remove_file(&socket_path);
  let listener = UnixListener::bind(&socket_path)?;
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&socket_path) {
      let mut perms = meta.permissions();
      perms.set_mode(0o600);
      if let Err(e) = std::fs::set_permissions(&socket_path, perms) {
        tracing::warn!(
          ?e,
          "failed to set permissions on {}",
          &socket_path.display()
        );
      }
    }
  }
  Ok(listener)
}
