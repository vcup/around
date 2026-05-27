//! IPC server: Unix domain socket with JSON newline-delimited protocol.
//!
//! Accepts commands from a local CLI over a Unix domain socket.
//! Each connection processes commands sequentially; multiple concurrent
//! connections are supported via per-connection tasks.

use crate::pipeline::Engine;
use around_core::{AroundError, SampleSpec};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Shared playback state, updated by command handlers and readable via `status`.
#[derive(Debug, Clone, Default)]
pub struct PlaybackState {
  pub playing: bool,
  pub position_ms: u64,
  pub duration_ms: Option<u64>,
  pub track_path: Option<String>,
  pub format_name: Option<String>,
  pub output_format: Option<SampleSpec>,
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
pub async fn run_ipc_server(
  socket_path: &str,
  engine: Arc<Engine>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Remove any leftover socket from a previous run.
  let _ = std::fs::remove_file(socket_path);

  let listener = UnixListener::bind(socket_path)?;
  tracing::info!("IPC server listening on {}", socket_path);

  let state = Arc::new(Mutex::new(PlaybackState::default()));

  loop {
    let (stream, _) = listener.accept().await?;
    let eng = engine.clone();
    let st = state.clone();
    tokio::spawn(async move {
      if let Err(e) = handle_connection(stream, eng, st).await {
        tracing::error!(?e, "IPC connection error");
      }
    });
  }
}

// ---------------------------------------------------------------------------
// Per-connection loop
// ---------------------------------------------------------------------------

async fn handle_connection(
  stream: UnixStream,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
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

    let resp = handle_command(cmd, &engine, &state).await;
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
) -> IpcResponse {
  match cmd {
    IpcCommand::Play { path } => handle_play(engine, state, path).await,
    IpcCommand::Pause => handle_pause(state),
    IpcCommand::Resume => handle_resume(state),
    IpcCommand::Seek { position_ms } => handle_seek(state, position_ms),
    IpcCommand::Stop => handle_stop(engine, state),
    IpcCommand::Status => handle_status(state),
    IpcCommand::ListDecoders => handle_list_decoders(),
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
    st.position_ms = 0;
    st.format_name = Some("WAV".into());
    st.duration_ms = None;
  }

  // Offload the blocking `engine.play()` call via spawn_blocking so the
  // async runtime stays responsive.  Playback runs to completion in the
  // background; the IPC client gets an immediate response.
  let eng = engine.clone();
  let st_bg = state.clone();
  let path_bg = path;
  tokio::spawn(async move {
    let result = tokio::task::spawn_blocking(move || {
      let source = around_source_file::FileSource::new(PathBuf::from(&path_bg));
      eng.play(Box::new(source))
    })
    .await;

    let mut st = st_bg.lock().unwrap();
    st.playing = false;
    match result {
      Ok(Ok(handle)) => {
        st.output_format = Some(handle.output_format);
        st.duration_ms = handle.metadata.duration.map(|d| d.as_millis() as u64);
      }
      Ok(Err(e)) => {
        tracing::error!(?e, "playback error");
      }
      Err(_) => {
        tracing::error!("playback task panicked");
      }
    }
  });

  let mut resp = IpcResponse::ok();
  resp.track_id = Some(1);
  resp.state = Some("playing".into());
  resp
}

fn handle_pause(state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let st = state.lock().unwrap();
  let mut resp = IpcResponse::ok();
  resp.state = Some("paused".into());
  resp.position_ms = Some(st.position_ms);
  resp
}

fn handle_resume(state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let st = state.lock().unwrap();
  let mut resp = IpcResponse::ok();
  resp.state = Some("playing".into());
  resp.position_ms = Some(st.position_ms);
  resp
}

fn handle_seek(state: &Arc<Mutex<PlaybackState>>, position_ms: u64) -> IpcResponse {
  let mut st = state.lock().unwrap();
  st.position_ms = position_ms;
  let mut resp = IpcResponse::ok();
  resp.position_ms = Some(position_ms);
  resp
}

fn handle_stop(engine: &Arc<Engine>, state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  engine.stop();
  let mut st = state.lock().unwrap();
  st.playing = false;
  let mut resp = IpcResponse::ok();
  resp.state = Some("stopped".into());
  resp.position_ms = Some(st.position_ms);
  resp
}

fn handle_status(state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let st = state.lock().unwrap();
  let mut resp = IpcResponse::ok();
  resp.state = Some(if st.playing { "playing" } else { "stopped" }.into());
  resp.position_ms = Some(st.position_ms);

  if let Some(ref path) = st.track_path {
    resp.track = Some(serde_json::json!({
        "id": 1,
        "path": path,
        "format": st.format_name.as_deref().unwrap_or("unknown"),
        "duration_ms": st.duration_ms.unwrap_or(0),
    }));
  }

  resp
}

fn handle_list_decoders() -> IpcResponse {
  let mut resp = IpcResponse::ok();
  resp.decoders = Some(vec![serde_json::json!({
      "name": "wav-builtin",
      "formats": ["WAV"],
      "source": "builtin",
  })]);
  resp
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
