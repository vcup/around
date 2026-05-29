//! IPC server: Unix domain socket with JSON newline-delimited protocol.
//!
//! Accepts commands from a local CLI over a Unix domain socket.
//! Each connection processes commands sequentially; multiple concurrent
//! connections are supported via per-connection tasks.

use crate::extensions::ExtensionManager;
use crate::pipeline::Engine;
use around_core::{AroundError, SampleSpec};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Shared playback state, updated by command handlers and readable via `status`.
// Manual Default: state defaults to "stopped"
impl Default for PlaybackState {
  fn default() -> Self {
    Self {
      playing: false,
      position_ms: 0,
      duration_ms: None,
      track_path: None,
      format_name: None,
      output_format: None,
      seekable: false,
      device_lost: false,
      state: "stopped".into(),
    }
  }
}

#[derive(Debug, Clone)]
pub struct PlaybackState {
  pub playing: bool,
  pub position_ms: u64,
  pub duration_ms: Option<u64>,
  pub track_path: Option<String>,
  pub format_name: Option<String>,
  pub output_format: Option<SampleSpec>,
  pub seekable: bool,
  pub device_lost: bool,
  pub state: String,
}

/// Incoming IPC command, tagged by the `command` field in JSON.
#[derive(Debug, Deserialize)]
#[serde(tag = "command")]
pub enum IpcCommand {
  #[serde(rename = "play")]
  Play { path: String },
  #[serde(rename = "pause")]
  Pause,
  #[serde(rename = "resume")]
  Resume,
  #[serde(rename = "seek")]
  Seek { position_ms: u64 },
  #[serde(rename = "stop")]
  Stop,
  #[serde(rename = "status")]
  Status,
  #[serde(rename = "list_decoders")]
  ListDecoders,
  #[serde(rename = "load_decoder")]
  LoadDecoder { path: String },
  #[serde(rename = "load_decoder_bytes")]
  LoadDecoderBytes { data: String, name: String },
  #[serde(rename = "cleanup")]
  Cleanup,
}

/// Outgoing IPC response; all fields optional except `status`.
#[derive(Debug, Serialize)]
pub struct IpcResponse {
  pub status: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub state: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub position_ms: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub code: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub message: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track: Option<serde_json::Value>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub decoders: Option<Vec<serde_json::Value>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub device_lost: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub removed_files: Option<Vec<String>>,
}

impl IpcResponse {
  pub fn ok() -> Self {
    Self {
      status: "ok".into(),
      state: None,
      position_ms: None,
      code: None,
      message: None,
      track_id: None,
      track: None,
      decoders: None,
      removed_files: None,
      device_lost: None,
    }
  }

  pub fn error(code: &str, msg: &str) -> Self {
    Self {
      status: "error".into(),
      state: None,
      position_ms: None,
      code: Some(code.into()),
      message: Some(msg.into()),
      track_id: None,
      track: None,
      decoders: None,
      removed_files: None,
      device_lost: None,
    }
  }
}

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Bind a Unix domain socket at `socket_path` and serve IPC commands forever.
///
/// Stale socket files are removed before binding.  Each accepted connection
/// is handled in its own tokio task so that multiple clients can interact
/// with the engine concurrently.
///
/// The accept loop exits when `engine.is_shutdown()` returns true (set by the
/// play thread after playback completes), allowing a clean join.
pub async fn run_ipc_server(
  socket_path: &str,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Check if an instance is already running on this socket.
  if let Ok(stream) = tokio::net::UnixStream::connect(socket_path).await {
    drop(stream);
    return Err(
      format!(
        "another engine instance is already running on {}",
        socket_path
      )
      .into(),
    );
  }

  // Remove stale socket file if connection was refused.
  let _ = std::fs::remove_file(socket_path);

  let listener = UnixListener::bind(socket_path)?;

  // Set socket permissions to 0600.
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(socket_path) {
      let mut perms = meta.permissions();
      perms.set_mode(0o600);
      let _ = std::fs::set_permissions(socket_path, perms);
    }
  }

  tracing::info!("IPC server listening on {}", socket_path);

  let ext_mgr = Arc::new(ExtensionManager::new());

  loop {
    if engine.is_shutdown() {
      tracing::info!("IPC server shutting down");
      break;
    }
    let (stream, _) = listener.accept().await?;
    let eng = engine.clone();
    let st = state.clone();
    let ext = ext_mgr.clone();
    tokio::spawn(async move {
      if let Err(e) = handle_connection(stream, eng, st, ext).await {
        tracing::error!(?e, "IPC connection error");
      }
    });
  }

  Ok(())
}

// ---------------------------------------------------------------------------
// Per-connection loop
// ---------------------------------------------------------------------------

async fn handle_connection(
  stream: UnixStream,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
  ext_mgr: Arc<ExtensionManager>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let (reader, mut writer) = stream.into_split();
  let mut lines = BufReader::new(reader).lines();

  while let Some(line) = lines.next_line().await? {
    let line = line.trim().to_string();
    if line.is_empty() {
      continue;
    }

    let cmd: IpcCommand = match serde_json::from_str(&line) {
      Ok(c) => c,
      Err(e) => {
        let resp = IpcResponse::error("PARSE_ERROR", &e.to_string());
        send_response(&mut writer, &resp).await?;
        continue;
      }
    };

    let resp = handle_command(cmd, &engine, &state, &ext_mgr).await;
    send_response(&mut writer, &resp).await?;
  }

  Ok(())
}

/// Serialise `resp` as a newline-terminated JSON line and write it to `writer`.
async fn send_response(
  writer: &mut (impl tokio::io::AsyncWrite + Unpin),
  resp: &IpcResponse,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let mut json = serde_json::to_string(resp)?;
  json.push('\n');
  writer.write_all(json.as_bytes()).await?;
  Ok(())
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

async fn handle_command(
  cmd: IpcCommand,
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  ext_mgr: &Arc<ExtensionManager>,
) -> IpcResponse {
  match cmd {
    IpcCommand::Play { path } => handle_play(engine, state, path).await,
    IpcCommand::Pause => handle_pause(engine, state),
    IpcCommand::Resume => handle_resume(engine, state),
    IpcCommand::Seek { position_ms } => handle_seek(engine, state, position_ms),
    IpcCommand::Stop => handle_stop(engine, state),
    IpcCommand::Status => handle_status(state),
    IpcCommand::ListDecoders => handle_list_decoders(ext_mgr),
    IpcCommand::LoadDecoder { path } => handle_load_decoder(ext_mgr, &path),
    IpcCommand::LoadDecoderBytes { data, name } => handle_load_decoder_bytes(ext_mgr, &data, &name),
    IpcCommand::Cleanup => handle_cleanup(),
  }
}

// ---------------------------------------------------------------------------
// Individual command handlers
// ---------------------------------------------------------------------------

async fn handle_play(
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  path: String,
) -> IpcResponse {
  // Fast-fail: check file existence synchronously per IPC contract.
  if std::fs::metadata(&path).is_err() {
    return IpcResponse::error("FILE_NOT_FOUND", &format!("No such file: {}", path));
  }

  // Update state before spawning — the background task will refine it on
  // completion (or failure).
  {
    let mut st = state.lock().unwrap();
    st.track_path = Some(path.clone());
    st.playing = true;
    st.state = "buffering".into();
    st.format_name = None; // engine.play() will populate on successful codec open
    st.device_lost = false;
    st.duration_ms = None;
  }

  // Offload the blocking `engine.play()` call via spawn_blocking so the
  // async runtime stays responsive.  Playback runs to completion in the
  // background; the IPC client gets an immediate response.
  //
  // The engine.play() call now receives the shared state and updates
  // position/state in real-time — no need for post-completion stitching.
  let eng = engine.clone();
  let st_play = Arc::clone(state);
  let st_err = Arc::clone(state);
  let path_bg = path;
  tokio::spawn(async move {
    let result = tokio::task::spawn_blocking(move || {
      let source = around_source_file::FileSource::new(PathBuf::from(&path_bg));
      eng.play(Box::new(source), st_play)
    })
    .await;

    match result {
      Ok(Ok(_handle)) => {
        // State is already updated to "stopped" by engine.play() on exit.
        tracing::info!("playback finished");
      }
      Ok(Err(e)) => {
        tracing::error!(?e, "playback error");
        if let Ok(mut st) = st_err.lock() {
          st.state = "error".into();
          st.playing = false;
        }
      }
      Err(_) => {
        tracing::error!("playback task panicked");
        if let Ok(mut st) = st_err.lock() {
          st.state = "error".into();
          st.playing = false;
        }
      }
    }
  });

  let mut resp = IpcResponse::ok();
  resp.track_id = Some(1);
  resp.state = Some("buffering".into());
  resp
}

fn handle_pause(engine: &Arc<Engine>, state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let mut st = state.lock().unwrap();
  if !st.playing {
    return IpcResponse::error("NO_TRACK", "no active playback to pause");
  }
  engine.pause();
  st.state = "paused".into();
  let mut resp = IpcResponse::ok();
  resp.state = Some("paused".into());
  resp.position_ms = Some(st.position_ms);
  resp
}

fn handle_resume(engine: &Arc<Engine>, state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let mut st = state.lock().unwrap();
  if !st.playing {
    return IpcResponse::error("NO_TRACK", "no active playback to resume");
  }
  engine.resume();
  st.state = "playing".into();
  let mut resp = IpcResponse::ok();
  resp.state = Some("playing".into());
  resp.position_ms = Some(st.position_ms);
  resp
}

fn handle_seek(
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  position_ms: u64,
) -> IpcResponse {
  let mut st = state.lock().unwrap();
  if !st.playing {
    return IpcResponse::error("NO_TRACK", "no active playback to seek");
  }
  if !st.seekable {
    return IpcResponse::error("NOT_SUPPORTED", "current source does not support seeking");
  }
  engine.seek(position_ms);
  st.position_ms = position_ms;
  st.state = "buffering".into();
  let mut resp = IpcResponse::ok();
  resp.position_ms = Some(position_ms);
  resp
}

fn handle_stop(engine: &Arc<Engine>, state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  engine.stop();
  let mut st = state.lock().unwrap();
  st.playing = false;
  st.state = "stopped".into();
  let mut resp = IpcResponse::ok();
  resp.state = Some("stopped".into());
  resp.position_ms = Some(st.position_ms);
  resp
}

fn handle_status(state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let st = state.lock().unwrap();
  let mut resp = IpcResponse::ok();
  resp.state = Some(st.state.clone());
  resp.position_ms = Some(st.position_ms);

  if let Some(ref path) = st.track_path {
    resp.track = Some(serde_json::json!({
        "id": 1,
        "path": path,
        "format": st.format_name.as_deref().unwrap_or("unknown"),
        "duration_ms": st.duration_ms.unwrap_or(0),
    }));
  }
  resp.device_lost = Some(st.device_lost);

  resp
}

fn handle_cleanup() -> IpcResponse {
  let mut removed = Vec::new();
  if let Ok(tmp_dir) = std::env::var("TMPDIR")
    .or_else(|_| std::env::var("TEMP"))
    .or_else(|_| std::env::temp_dir().into_os_string().into_string())
  {
    let dir = std::path::Path::new(&tmp_dir);
    if let Ok(entries) = std::fs::read_dir(dir) {
      for entry in entries.flatten() {
        let path = entry.path();
        if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
          if (fname.starts_with("around-decoder-") || fname.starts_with("around_decoder_"))
            && std::fs::remove_file(&path).is_ok()
          {
            removed.push(path.display().to_string());
          }
        }
      }
    }
  }
  let mut resp = IpcResponse::ok();
  resp.message = Some(serde_json::json!({"removed_files": removed}).to_string());
  resp
}

fn handle_list_decoders(ext_mgr: &Arc<ExtensionManager>) -> IpcResponse {
  let decoders = ext_mgr.list_decoders();
  let mut resp = IpcResponse::ok();
  resp.decoders = Some(
    decoders
      .iter()
      .map(|d| {
        serde_json::json!({
            "name": d.name,
            "formats": d.formats,
            "source": d.source,
        })
      })
      .collect(),
  );
  resp
}

fn handle_load_decoder(ext_mgr: &Arc<ExtensionManager>, path: &str) -> IpcResponse {
  match ext_mgr.load_path(std::path::Path::new(path)) {
    Ok(info) => {
      let mut resp = IpcResponse::ok();
      resp.decoders = Some(vec![serde_json::json!({
          "name": info.name,
          "formats": info.formats,
          "source": info.source,
          "path": info.path.map(|p| p.display().to_string()),
      })]);
      resp
    }
    Err(e) => {
      let (code, msg) = map_decoder_error(&e);
      IpcResponse::error(code, &msg)
    }
  }
}

fn handle_load_decoder_bytes(
  ext_mgr: &Arc<ExtensionManager>,
  data: &str,
  name: &str,
) -> IpcResponse {
  match ext_mgr.load_bytes(data.as_bytes(), name) {
    Ok(info) => {
      let mut resp = IpcResponse::ok();
      resp.decoders = Some(vec![serde_json::json!({
          "name": info.name,
          "formats": info.formats,
          "source": info.source,
      })]);
      resp
    }
    Err(e) => {
      let (code, msg) = map_decoder_error(&e);
      IpcResponse::error(code, &msg)
    }
  }
}

/// Map a decoder-related error to an IPC error code.
fn map_decoder_error(e: &AroundError) -> (&'static str, String) {
  match e {
    AroundError::DecoderLoadFailed { .. } => ("DECODER_LOAD_FAILED", e.to_string()),
    _ => ("INTERNAL", e.to_string()),
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map an [`AroundError`] to the IPC error code expected by the contract.
fn _map_engine_error(e: &AroundError) -> IpcResponse {
  let code = match e {
    AroundError::FileNotFound { .. } => ("FILE_NOT_FOUND", e.to_string()),
    AroundError::UnsupportedFormat { .. } => ("UNSUPPORTED_FORMAT", e.to_string()),
    AroundError::DecodeError { .. } => ("DECODE_ERROR", e.to_string()),
    AroundError::NoTrack => ("NO_TRACK", e.to_string()),
    AroundError::DecoderLoadFailed { .. } => ("DECODER_LOAD_FAILED", e.to_string()),
    _ => ("INTERNAL", e.to_string()),
  };
  IpcResponse::error(code.0, &code.1)
}
