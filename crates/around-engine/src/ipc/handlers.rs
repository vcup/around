// Clippy: Mutex poisoning panics are intentional — they indicate unrecoverable bugs.
#![allow(clippy::expect_used)]
use crate::ipc::types::{
  CodecDescriptor, ErrorCode, IpcResponse, PlaybackState, TrackInfo, TrackState,
};
use crate::pipeline::Engine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) fn handle_play(
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  path: String,
) -> IpcResponse {
  // Fast-fail: check file existence synchronously per IPC contract.
  if std::fs::metadata(&path).is_err() {
    return IpcResponse::error(ErrorCode::FileNotFound, &format!("No such file: {}", path));
  }

  // Update state before spawning — the background task will refine it on
  // completion (or failure).
  // FR-009: VLC-style replace — stop any active playback before starting new.
  engine.stop();
  let epoch = {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.epoch += 1;
    st.track_path = Some(path.clone());
    st.playing = true;
    st.state = TrackState::Buffering;
    st.format_name = None; // engine.play() will populate on successful codec open
    st.device_lost = false;
    st.duration_ms = None;
    st.epoch
  };

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
          if st.epoch == epoch {
            st.state = TrackState::Error;
            st.playing = false;
          }
        }
      }
      Err(_) => {
        tracing::error!("playback task panicked");
        if let Ok(mut st) = st_err.lock() {
          if st.epoch == epoch {
            st.state = TrackState::Error;
            st.playing = false;
          }
        }
      }
    }
  });

  let mut resp = IpcResponse::ok();
  resp.track_id = Some(1);
  resp.state = Some(TrackState::Buffering);
  resp
}

pub(crate) fn handle_pause(engine: &Arc<Engine>, state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
  if !st.playing {
    return IpcResponse::error(ErrorCode::NoTrack, "no active playback to pause");
  }
  engine.pause();
  st.state = TrackState::Paused;
  let mut resp = IpcResponse::ok();
  resp.state = Some(TrackState::Paused);
  resp.position_ms = Some(st.position_ms);
  resp
}

pub(crate) fn handle_resume(
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
) -> IpcResponse {
  let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
  if !st.playing {
    return IpcResponse::error(ErrorCode::NoTrack, "no active playback to resume");
  }
  engine.resume();
  st.state = TrackState::Playing;
  let mut resp = IpcResponse::ok();
  resp.state = Some(TrackState::Playing);
  resp.position_ms = Some(st.position_ms);
  resp
}

pub(crate) fn handle_seek(
  engine: &Arc<Engine>,
  state: &Arc<Mutex<PlaybackState>>,
  position_ms: u64,
) -> IpcResponse {
  let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
  if !st.playing {
    return IpcResponse::error(ErrorCode::NoTrack, "no active playback to seek");
  }
  if !st.seekable {
    return IpcResponse::error(
      ErrorCode::NotSupported,
      "current source does not support seeking",
    );
  }

  if let Some(dur) = st.duration_ms {
    if position_ms > dur {
      return IpcResponse::error(
        ErrorCode::InvalidPosition,
        "seek position exceeds track duration",
      );
    }
  }

  engine.seek(position_ms);
  st.position_ms = position_ms;
  st.state = TrackState::Buffering;
  let mut resp = IpcResponse::ok();
  resp.position_ms = Some(position_ms);
  resp
}

pub(crate) fn handle_stop(engine: &Arc<Engine>, state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  engine.stop();
  let mut st = state.lock().expect("state lock poisoned");
  st.playing = false;
  st.state = TrackState::Stopped;
  let mut resp = IpcResponse::ok();
  resp.state = Some(TrackState::Stopped);
  resp.position_ms = Some(st.position_ms);
  resp
}

pub(crate) fn handle_status(state: &Arc<Mutex<PlaybackState>>) -> IpcResponse {
  let st = state.lock().expect("state lock poisoned");
  let mut resp = IpcResponse::ok();
  resp.state = Some(st.state);
  resp.position_ms = Some(st.position_ms);
  resp.track_id = Some(1);

  if let Some(ref path) = st.track_path {
    resp.track = Some(TrackInfo {
      id: 1,
      path: path.clone(),
      format: st.format_name.clone().unwrap_or_else(|| "unknown".into()),
      duration_ms: st.duration_ms.unwrap_or(0),
    });
  }
  resp.device_lost = Some(st.device_lost);

  resp
}

/// Scan temp_dir for `around_codec_*` files NOT from our PID, remove them.
/// Returns display paths of removed files.
pub(crate) fn scan_and_remove_stale_temp_files() -> Vec<String> {
  let mut removed = Vec::new();
  let current_pid = std::process::id();
  let our_prefix = format!("around_codec_{}_", current_pid);

  let tmp_dir = std::env::var("TMPDIR")
    .map(std::path::PathBuf::from)
    .or_else(|_| std::env::var("TEMP").map(std::path::PathBuf::from))
    .unwrap_or_else(|_| std::env::temp_dir());

  if let Ok(entries) = std::fs::read_dir(&tmp_dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
        if fname.starts_with("around_codec_")
          && !fname.starts_with(&our_prefix)
          && std::fs::remove_file(&path).is_ok()
        {
          removed.push(path.display().to_string());
        }
      }
    }
  }
  removed
}

pub(crate) fn handle_cleanup() -> IpcResponse {
  let removed: Vec<String> = scan_and_remove_stale_temp_files();

  let mut resp = IpcResponse::ok();
  resp.removed_files = Some(removed);
  if resp.removed_files.as_ref().is_some_and(|v| !v.is_empty()) {
    resp.message = Some(format!(
      "removed {} file(s)",
      resp.removed_files.as_ref().map_or(0, |v| v.len())
    ));
  }
  resp
}

pub(crate) fn handle_list_codecs() -> IpcResponse {
  let framework = around_extensions::Framework::instance();
  let entries = framework.index_entries();
  let codecs: Vec<CodecDescriptor> = entries
    .into_iter()
    .map(|(name, path)| CodecDescriptor {
      name,
      formats: Vec::new(),
      source: "scanned".into(),
      path: Some(path.to_string_lossy().into_owned()),
    })
    .collect();

  let mut resp = IpcResponse::ok();
  resp.codecs = Some(codecs);
  resp
}

pub(crate) fn handle_load_codec(path: &str) -> IpcResponse {
  match around_extensions::Framework::instance().load(path) {
    Ok(_lib) => {
      let mut resp = IpcResponse::ok();
      resp.codecs = Some(vec![CodecDescriptor {
        name: path.to_string(),
        formats: Vec::new(),
        source: "loaded".into(),
        path: Some(path.to_string()),
      }]);
      resp
    }
    Err(e) => {
      let (code, msg) = map_framework_error(&e);
      IpcResponse::error(code, &msg)
    }
  }
}

pub(crate) fn handle_load_codec_bytes(data: Vec<u8>) -> IpcResponse {
  use std::hash::{Hash, Hasher};
  let mut h = std::collections::hash_map::DefaultHasher::new();
  data.hash(&mut h);
  let hash = h.finish();

  let tmp_path = std::env::temp_dir().join(format!(
    "around_codec_{}_{:016x}.{}",
    std::process::id(),
    hash,
    std::env::consts::DLL_EXTENSION
  ));

  if let Err(e) = std::fs::write(&tmp_path, &data) {
    return IpcResponse::error(
      ErrorCode::CodecLoadFailed,
      &format!("failed to write temp file: {}", e),
    );
  }

  let name = tmp_path
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or("unknown");

  match around_extensions::Framework::instance().load(name) {
    Ok(_lib) => {
      let mut resp = IpcResponse::ok();
      resp.codecs = Some(vec![CodecDescriptor {
        name: name.to_string(),
        formats: Vec::new(),
        source: "bytes".into(),
        path: Some(tmp_path.to_string_lossy().into_owned()),
      }]);
      resp
    }
    Err(e) => {
      let _ = std::fs::remove_file(&tmp_path);
      let (code, msg) = map_framework_error(&e);
      IpcResponse::error(code, &msg)
    }
  }
}

fn map_framework_error(e: &around_extensions::FrameworkError) -> (ErrorCode, String) {
  match e {
    around_extensions::FrameworkError::NotFound(_) => (ErrorCode::CodecLoadFailed, e.to_string()),
    _ => (ErrorCode::CodecLoadFailed, e.to_string()),
  }
}

pub(crate) fn handle_shutdown(engine: &Arc<Engine>) -> IpcResponse {
  engine.shutdown();
  IpcResponse::ok()
}
