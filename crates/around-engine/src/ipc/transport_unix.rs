//! Unix domain socket transport (Linux/macOS).

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

const SOCKET_FILE_NAME: &str = "around.sock";

/// Ordered discovery paths shared by the server and CLI.
///
/// An explicit path is exclusive. Otherwise try the runtime directory, the
/// process temporary directory, and `/tmp`, omitting empty or duplicate paths.
pub fn socket_paths(custom_path: Option<&Path>) -> impl Iterator<Item = PathBuf> {
  let paths = if let Some(path) = custom_path {
    [Some(path.to_path_buf()), None, None]
  } else {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
      .filter(|dir| !dir.is_empty())
      .map(|dir| PathBuf::from(dir).join("around").join(SOCKET_FILE_NAME));
    let temporary = std::env::temp_dir().join(SOCKET_FILE_NAME);
    let system = PathBuf::from("/tmp").join(SOCKET_FILE_NAME);
    let system = (runtime.as_ref() != Some(&system) && temporary != system).then_some(system);
    let temporary = (runtime.as_ref() != Some(&temporary)).then_some(temporary);
    [runtime, temporary, system]
  };
  paths.into_iter().flatten()
}

fn ensure_socket_parent(dir: &Path) -> io::Result<()> {
  if !dir.exists() {
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
  } else if !dir.is_dir() {
    return Err(io::Error::new(
      io::ErrorKind::NotADirectory,
      "unix socket parent is not a directory",
    ));
  }
  Ok(())
}

fn socket_in_use_error() -> io::Error {
  io::Error::new(io::ErrorKind::AddrInUse, "unix socket already in use")
}

/// Keep the lock inode stable across restarts: unlinking the lock file would
/// let two processes lock different inodes for the same socket pathname.
fn lock_socket_path(socket_path: &Path) -> io::Result<File> {
  let mut lock_path = socket_path.as_os_str().to_os_string();
  lock_path.push(".lock");
  let lock = OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .mode(0o600)
    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
    .open(PathBuf::from(lock_path))?;
  // SAFETY: the descriptor belongs to the live File; flock retains no pointer.
  // Nonblocking acquisition also arbitrates contenders before a listener exists.
  if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
    let error = io::Error::last_os_error();
    return Err(if error.kind() == io::ErrorKind::WouldBlock {
      socket_in_use_error()
    } else {
      error
    });
  }
  Ok(lock)
}

/// Own binding, cleanup and the cross-process lock as one lifetime.
/// The lock stays held until after Drop removes our endpoint and closes it.
pub(crate) struct BoundUnixSocket {
  listener: UnixListener,
  path: PathBuf,
  identity: (u64, u64),
  _lock: File,
}

impl BoundUnixSocket {
  pub(crate) fn path(&self) -> &Path {
    &self.path
  }

  pub(crate) async fn accept(&self) -> io::Result<UnixStream> {
    self.listener.accept().await.map(|(stream, _)| stream)
  }
}

impl Drop for BoundUnixSocket {
  fn drop(&mut self) {
    // FR-005: clean up the actual bound endpoint, never a replacement that
    // another program installed after externally unlinking our socket.
    if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
      if metadata.file_type().is_socket() && (metadata.dev(), metadata.ino()) == self.identity {
        let _ = std::fs::remove_file(&self.path);
      }
    }
  }
}

async fn bind_unix_socket_at(
  socket_path: &Path,
  parent_dir: Option<&Path>,
) -> io::Result<BoundUnixSocket> {
  if let Some(dir) = parent_dir {
    ensure_socket_parent(dir)?;
  }
  // Hold this through listening and cleanup, not just the stale-file probe.
  // Otherwise two contenders can both observe ConnectionRefused and unlink
  // a socket that the other has just rebound.
  let lock = lock_socket_path(socket_path)?;

  // A missing path is left alone: bind() atomically arbitrates new instances.
  // Never unlink ordinary files or symlinks, even if connect() would fail.
  match std::fs::symlink_metadata(socket_path) {
    Ok(metadata) => {
      if !metadata.file_type().is_socket() {
        return Err(socket_in_use_error());
      }
      match UnixStream::connect(socket_path).await {
        Ok(_) => return Err(socket_in_use_error()),
        Err(err) if err.kind() == io::ErrorKind::ConnectionRefused => {
          // Do not remove a different endpoint installed during the probe.
          let current = std::fs::symlink_metadata(socket_path)?;
          if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
            return Err(socket_in_use_error());
          }
          std::fs::remove_file(socket_path)?;
        }
        // Permission and other probe failures do not prove the socket stale.
        Err(err) => return Err(err),
      }
    }
    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
    Err(err) => return Err(err),
  }

  let listener = UnixListener::bind(socket_path)?;
  let metadata = std::fs::symlink_metadata(socket_path)?;
  let bound = BoundUnixSocket {
    listener,
    path: socket_path.to_path_buf(),
    identity: (metadata.dev(), metadata.ino()),
    _lock: lock,
  };
  // Failure drops the owned binding and cleans up; never accept commands
  // without owner-only permissions.
  std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
  Ok(bound)
}

/// Bind an explicit path, or try each standard discovery path in order.
/// A live socket or non-socket collision is terminal, not a reason to fall back.
pub(crate) async fn bind_unix_socket(custom_path: Option<&Path>) -> io::Result<BoundUnixSocket> {
  let mut last_error = io::Error::other("no Unix socket path available");
  for socket_path in socket_paths(custom_path) {
    let parent = if custom_path.is_none() {
      socket_path.parent()
    } else {
      None
    };
    match bind_unix_socket_at(&socket_path, parent).await {
      Ok(listener) => return Ok(listener),
      Err(err) if err.kind() == io::ErrorKind::AddrInUse => return Err(err),
      Err(err) => last_error = err,
    }
  }
  Err(last_error)
}
