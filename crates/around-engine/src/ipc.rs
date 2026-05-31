//! IPC server: TCP on localhost with JSON newline-delimited protocol.
//!
//! Accepts commands from a local CLI over a TCP connection on 127.0.0.1.
//! Each connection processes commands sequentially; multiple concurrent
//! connections are supported via per-connection tasks.

use crate::extensions::ExtensionManager;
use crate::ipc_codec::JsonLineCodec;
use crate::ipc_types::{IpcCommand, IpcResponse, PlaybackState};
use crate::pipeline::Engine;
use around_core::AroundError;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Bind a TCP listener on `127.0.0.1:0` (random port) and serve IPC commands forever.
///
/// The chosen port is written to `port_file`.  On startup, if `port_file`
/// already exists the port is read and a connection attempt is made to
/// check for a live instance — if one is found the server refuses to start.
///
/// Each accepted connection is handled in its own tokio task so that
/// multiple clients can interact with the engine concurrently.
///
/// The accept loop exits when `engine.is_shutdown()` returns true (set by
/// the play thread after playback completes), allowing a clean join.
pub async fn run_ipc_server(
  port_file: &str,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  // Check if an instance is already running by reading the port file.
  if let Ok(port_str) = std::fs::read_to_string(port_file) {
    if let Ok(port) = port_str.trim().parse::<u16>() {
      let addr = format!("127.0.0.1:{}", port);
      if TcpStream::connect(&addr).await.is_ok() {
        return Err(
          format!(
            "another engine instance is already running on port {}",
            port
          )
          .into(),
        );
      }
    }
    // Stale port file — connection was refused or port invalid.
    let _ = std::fs::remove_file(port_file);
  }

  let listener = TcpListener::bind("127.0.0.1:0").await?;
  let port = listener.local_addr()?.port();

  std::fs::write(port_file, port.to_string())?;

  tracing::info!("IPC server listening on 127.0.0.1:{}", port);

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
  stream: TcpStream,
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
  ext_mgr: Arc<ExtensionManager>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let (reader, mut writer) = stream.into_split();
  let codec = JsonLineCodec;
  let mut buf_reader = BufReader::new(reader);

  loop {
    let cmd = match codec.read_command(&mut buf_reader).await {
      Ok(c) => c,
      Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
      Err(e) => return Err(e.into()),
    };

    let resp = handle_command(cmd, &engine, &state, &ext_mgr).await;
    codec.write_response(&mut writer, &resp).await?;
  }
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
