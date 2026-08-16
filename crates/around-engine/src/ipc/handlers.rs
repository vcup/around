use crate::ipc::types::{CodecDescriptor, ErrorCode, IpcResponse, StreamStatus, TrackInfo};
use crate::pipeline::Engine;
use std::path::PathBuf;
use std::sync::Arc;

/// Resolve an optional stream_id to a concrete stream ID.
/// If None, uses the sole active stream. Returns an error response if no stream found.
// IpcResponse is intentionally large (carries all response fields in one type).
// Boxing fields would add indirection without benefit. Expected removal: when
// response types are split per-command (Post-ADR-0003 IPC rationalization).
#[expect(
  clippy::result_large_err,
  reason = "IpcResponse is a unified wire type; boxing adds indirection until responses split per command"
)]
fn resolve_stream_id(engine: &Arc<Engine>, stream_id: Option<u64>) -> Result<u64, IpcResponse> {
  match stream_id {
    Some(id) => Ok(id),
    None => engine
      .sole_stream_id()
      .ok_or_else(|| IpcResponse::error(ErrorCode::NoTrack, "no active stream")),
  }
}

pub(crate) fn handle_play(
  engine: &Arc<Engine>,
  path: String,
  _stream_id: Option<u64>,
) -> IpcResponse {
  // Fast-fail: check file existence synchronously per IPC contract.
  if std::fs::metadata(&path).is_err() {
    return IpcResponse::error(ErrorCode::FileNotFound, &format!("No such file: {path}"));
  }

  // Prepare the source — opens codec, creates stream, returns metadata.
  let source = around_source_file::FileSource::new(PathBuf::from(&path));
  let prepared = match engine.prepare(Box::new(source)) {
    Ok(p) => p,
    Err(e) => {
      return IpcResponse::error(ErrorCode::CodecLoadFailed, &e.to_string());
    }
  };

  let stream_id = prepared.stream_id;
  // Spawn the decode loop.
  let eng = engine.clone();
  #[cfg(feature = "async-decode")]
  tokio::spawn(async move {
    if let Err(e) = eng.run_stream_spawn(stream_id) {
      tracing::error!(?e, "async playback error");
    }
  });
  #[cfg(not(feature = "async-decode"))]
  tokio::spawn(async move {
    if let Err(e) = eng.run_stream(stream_id) {
      tracing::error!(?e, "playback error");
    }
  });

  let mut resp = IpcResponse::ok();
  resp.stream_id = Some(stream_id);
  resp.track_id = Some(stream_id);
  resp.track = Some(TrackInfo {
    id: stream_id,
    path,
    format: prepared.codec_name,
    duration_ms: prepared.duration_ms.unwrap_or(0),
  });
  resp.seekable = Some(prepared.seekable);
  resp
}

pub(crate) fn handle_pause(engine: &Arc<Engine>, stream_id: Option<u64>) -> IpcResponse {
  match resolve_stream_id(engine, stream_id) {
    Ok(sid) => {
      engine.pause_stream(sid);
      let mut resp = IpcResponse::ok();
      if let Some(ss) = engine.stream_state(sid) {
        resp.position_ms = Some(ss.position_ms.load(std::sync::atomic::Ordering::SeqCst));
      }
      resp
    }
    Err(resp) => resp,
  }
}

pub(crate) fn handle_resume(engine: &Arc<Engine>, stream_id: Option<u64>) -> IpcResponse {
  match resolve_stream_id(engine, stream_id) {
    Ok(sid) => {
      engine.resume_stream(sid);
      let mut resp = IpcResponse::ok();
      if let Some(ss) = engine.stream_state(sid) {
        resp.position_ms = Some(ss.position_ms.load(std::sync::atomic::Ordering::SeqCst));
      }
      resp
    }
    Err(resp) => resp,
  }
}

pub(crate) fn handle_seek(
  engine: &Arc<Engine>,
  position_ms: u64,
  stream_id: Option<u64>,
) -> IpcResponse {
  let sid = match resolve_stream_id(engine, stream_id) {
    Ok(id) => id,
    Err(resp) => return resp,
  };

  match engine.seek_stream(sid, position_ms) {
    Ok(pos) => {
      let mut resp = IpcResponse::ok();
      resp.position_ms = Some(pos);
      resp
    }
    Err(e) => {
      let (code, msg) = match e {
        crate::pipeline::SeekError::StreamNotFound => (ErrorCode::NoTrack, "stream not found"),
        crate::pipeline::SeekError::NotSeekable => (
          ErrorCode::NotSupported,
          "current source does not support seeking",
        ),
        crate::pipeline::SeekError::StreamEnded => (
          ErrorCode::InvalidPosition,
          "seek position exceeds track duration or stream ended",
        ),
      };
      IpcResponse::error(code, msg)
    }
  }
}

pub(crate) fn handle_stop(engine: &Arc<Engine>, stream_id: Option<u64>) -> IpcResponse {
  match stream_id {
    Some(sid) => {
      engine.stop_stream(Some(sid));
      IpcResponse::ok()
    }
    None => {
      engine.stop();
      IpcResponse::ok()
    }
  }
}

pub(crate) fn handle_status(engine: &Arc<Engine>, stream_id: Option<u64>) -> IpcResponse {
  let mut resp = IpcResponse::ok();

  match stream_id {
    Some(sid) => {
      // Single-stream status
      if let Some(ss) = engine.stream_state(sid) {
        resp.state = Some(ss.status());
        resp.position_ms = Some(ss.position_ms.load(std::sync::atomic::Ordering::SeqCst));
        resp.stream_id = Some(sid);
        resp.device_lost = Some(ss.device_lost.load(std::sync::atomic::Ordering::SeqCst));
        let path = engine.stream_source_path(sid);
        let codec = engine.stream_codec_name(sid);
        let duration = engine.stream_duration_ms(sid);
        let s = engine.stream_seekable(sid);
        resp.seekable = s;
        resp.content_type = engine.stream_content_type(sid);
        if let Some(p) = path {
          resp.track = Some(TrackInfo {
            id: sid,
            path: p,
            format: codec.unwrap_or_else(|| "unknown".into()),
            duration_ms: duration.unwrap_or(0),
          });
        }
      } else {
        return IpcResponse::error(ErrorCode::NoTrack, "stream not found");
      }
    }
    None => {
      // All-streams status
      let ids = engine.stream_ids();
      let mut streams: Vec<StreamStatus> = Vec::new();
      for &id in &ids {
        if let Some(ss) = engine.stream_state(id) {
          let seekable = engine.stream_seekable(id).unwrap_or(false);
          let path = engine.stream_source_path(id);
          let codec = engine.stream_codec_name(id);
          let duration = engine.stream_duration_ms(id);
          streams.push(StreamStatus {
            stream_id: id,
            status: ss.status(),
            position_ms: ss.position_ms.load(std::sync::atomic::Ordering::SeqCst),
            seekable,
            device_lost: ss.device_lost.load(std::sync::atomic::Ordering::SeqCst),
            track: path.map(|p| TrackInfo {
              id,
              path: p,
              format: codec.unwrap_or_else(|| "unknown".into()),
              duration_ms: duration.unwrap_or(0),
            }),
            content_type: engine.stream_content_type(id),
          });
        }
      }
      resp.streams = Some(streams);

      // Backward compat: populate legacy flat fields from sole stream.
      if let Some(sole) = engine.sole_stream_id() {
        if let Some(ss) = engine.stream_state(sole) {
          resp.state = Some(ss.status());
          resp.position_ms = Some(ss.position_ms.load(std::sync::atomic::Ordering::SeqCst));
          resp.stream_id = Some(sole);
          resp.device_lost = Some(ss.device_lost.load(std::sync::atomic::Ordering::SeqCst));
          resp.seekable = engine.stream_seekable(sole);
          resp.content_type = engine.stream_content_type(sole);
          let path = engine.stream_source_path(sole);
          let codec = engine.stream_codec_name(sole);
          let duration = engine.stream_duration_ms(sole);
          if let Some(p) = path {
            resp.track = Some(TrackInfo {
              id: sole,
              path: p,
              format: codec.unwrap_or_else(|| "unknown".into()),
              duration_ms: duration.unwrap_or(0),
            });
          }
        }
      }
    }
  }

  resp
}

/// Scan temp_dir for `around_codec_*` files NOT from our PID, remove them.
/// Returns display paths of removed files.
pub(crate) fn scan_and_remove_stale_temp_files() -> Vec<String> {
  let mut removed = Vec::new();
  let current_pid = std::process::id();
  let our_prefix = format!("around_codec_{current_pid}_");

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
      &format!("failed to write temp file: {e}"),
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

#[cfg(test)]
mod tests {
  #![expect(
    clippy::expect_used,
    reason = "committed fixture and freshly prepared stream are test invariants"
  )]
  use super::*;
  use crate::config::OutputDriver;

  #[test]
  fn status_preserves_source_content_type() {
    let mut config = crate::EngineConfig::load();
    config.output_driver = OutputDriver::Null;
    let engine = Arc::new(Engine::new(config));
    let fixture =
      std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/example.wav");
    let prepared = engine
      .prepare(Box::new(around_source_file::FileSource::new(fixture)))
      .expect("fixture must prepare");

    let single = handle_status(&engine, Some(prepared.stream_id));
    assert_eq!(single.content_type.as_deref(), Some("audio/wav"));

    let all = handle_status(&engine, None);
    let streams = all.streams.expect("all-stream status must include streams");
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].content_type.as_deref(), Some("audio/wav"));
  }
}
