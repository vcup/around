//! Audio pipeline: multi-stream engine (ADR-0005 compliant).
//!
//! Single `Engine` manages 0..N concurrent `Stream`s. Each stream has
//! independent state via all-Atomic `StreamState` and `CancellationToken`.

use crate::config::OutputDriver;
use crate::output_plan::{DeviceSelector, ModePreference, OutputPreference, RecoveryPolicy};
use crate::output_session::{null_adapter, OutputSession};
use around_audio_sdk::codec::{debug_track_stream, init_codec, SafeCodecRef, StreamInfo};
use around_core::state::PlaybackStatus;
use around_core::{AroundError, PcmEncoding, SampleSpec, Source, SourceCapabilities};
use crossbeam::channel;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// StreamId
// ---------------------------------------------------------------------------

pub type StreamId = u64;
static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);

fn next_stream_id() -> StreamId {
  NEXT_STREAM_ID.fetch_add(1, Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// StreamState — all-Atomic per-stream state
// ---------------------------------------------------------------------------

/// Atomic wrapper around `PlaybackStatus` for lock-free status transitions.
///
/// Satisfies ADR-0005: the public API uses `PlaybackStatus` directly
/// instead of raw `u8` conversions.
struct AtomicPlaybackStatus(AtomicU8);

impl AtomicPlaybackStatus {
  const fn new(s: PlaybackStatus) -> Self {
    Self(AtomicU8::new(s as u8))
  }

  fn load(&self, order: Ordering) -> PlaybackStatus {
    match self.0.load(order) {
      0 => PlaybackStatus::Playing,
      1 => PlaybackStatus::Paused,
      2 => PlaybackStatus::Stopped,
      3 => PlaybackStatus::Buffering,
      4 => PlaybackStatus::Error,
      _ => PlaybackStatus::Stopped,
    }
  }

  fn store(&self, s: PlaybackStatus, order: Ordering) {
    self.0.store(s as u8, order);
  }
}

pub struct StreamState {
  pub active: AtomicBool,
  pub position_ms: AtomicU64,
  status: AtomicPlaybackStatus,
  pub device_lost: AtomicBool,
}

impl StreamState {
  pub fn new() -> Self {
    Self {
      active: AtomicBool::new(false),
      position_ms: AtomicU64::new(0),
      status: AtomicPlaybackStatus::new(PlaybackStatus::Stopped),
      device_lost: AtomicBool::new(false),
    }
  }

  pub fn status(&self) -> PlaybackStatus {
    self.status.load(Ordering::SeqCst)
  }

  pub fn set_status(&self, s: PlaybackStatus) {
    self.status.store(s, Ordering::SeqCst);
  }
}

impl Default for StreamState {
  fn default() -> Self {
    Self::new()
  }
}

// ---------------------------------------------------------------------------
// Stream handle (stored in Engine)
// ---------------------------------------------------------------------------

/// Wrapper for an opaque codec stream pointer that is Send + Sync.
/// The pointer is only accessed through the codec vtable dispatch,
/// which is thread-safe per stabby's design.
#[derive(Clone, Copy)]
struct StreamPtr(*mut c_void);
// SAFETY: StreamPtr wraps *mut c_void which is neither Send nor Sync by default. The engine ensures that StreamPtr values are only accessed from the stream's owning thread or under appropriate synchronization. The raw pointer is never dereferenced concurrently without synchronization.
unsafe impl Send for StreamPtr {}
// SAFETY: See Send impl above. Same invariant — the engine ensures no concurrent mutable access through different threads.
unsafe impl Sync for StreamPtr {}

/// Owned pointer to the reader bridge used by a codec stream.
///
/// The codec may invoke its read/seek callbacks until `Codec::drop`, so this
/// allocation must outlive the opaque codec stream.
#[derive(Clone, Copy)]
struct ReaderPtr(*mut c_void);
// SAFETY: each ReaderPtr is owned by one ActiveStream and only accessed by
// that stream's serial decode loop.
unsafe impl Send for ReaderPtr {}
// SAFETY: shared access never dereferences the pointer; mutation is serialized.
unsafe impl Sync for ReaderPtr {}

struct ActiveStream {
  id: StreamId,
  state: Arc<StreamState>,
  cancel: CancellationToken,
  stream_ptr: Option<StreamPtr>,
  codec_ref: Option<SafeCodecRef<'static>>,
  reader_ctx: Option<ReaderPtr>,
  sample_rate: u32,
  channels: u8,
  #[allow(dead_code)]
  total_frames: u64,
  source_path: String,
  content_type: Option<String>,
  codec_name: String,
  duration_ms: Option<u64>,
  seekable: bool,
  source_format: SampleSpec,
  output_spec: Option<SampleSpec>,
  seek_tx: Option<channel::Sender<u64>>,
  seek_rx: Option<channel::Receiver<u64>>,
}

impl Drop for ActiveStream {
  fn drop(&mut self) {
    // The codec stream must be dropped before its reader callback context.
    if let (Some(StreamPtr(stream_ptr)), Some(codec_ref)) =
      (self.stream_ptr.take(), self.codec_ref.take())
    {
      codec_ref.drop(stream_ptr);
      debug_track_stream(false);
    }
    if let Some(reader_ctx) = self.reader_ctx.take() {
      drop_reader_context(reader_ctx);
    }
  }
}

// ---------------------------------------------------------------------------
// PreparedStream — returned by Engine::prepare()
// ---------------------------------------------------------------------------

pub struct PreparedStream {
  pub stream_id: StreamId,
  pub codec_name: String,
  pub duration_ms: Option<u64>,
  pub seekable: bool,
  pub source_format: SampleSpec,
}

// ---------------------------------------------------------------------------
// SeekError — returned by Engine::seek_stream()
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeekError {
  StreamNotFound,
  NotSeekable,
  StreamEnded,
}

impl std::fmt::Display for SeekError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      SeekError::StreamNotFound => write!(f, "stream not found"),
      SeekError::NotSeekable => write!(f, "stream is not seekable"),
      SeekError::StreamEnded => write!(f, "stream has ended or been stopped"),
    }
  }
}

impl std::error::Error for SeekError {}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct Engine {
  config: crate::config::EngineConfig,
  device_lost: Arc<AtomicBool>,
  streams: RwLock<HashMap<StreamId, ActiveStream>>,
  shutdown_token: CancellationToken,
}

pub struct PlaybackHandle {
  pub stream_id: StreamId,
}

trait ReadSeekSend: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> ReadSeekSend for T {}

struct ReaderBridge<R: Read + Seek + Send> {
  reader: R,
}

fn drop_reader_context(ReaderPtr(ctx): ReaderPtr) {
  if !ctx.is_null() {
    // SAFETY: ReaderPtr is created exactly once from Box::into_raw in
    // open_codec and consumed exactly once after the codec stream is dropped.
    unsafe {
      drop(Box::from_raw(
        ctx as *mut ReaderBridge<Box<dyn ReadSeekSend>>,
      ));
    }
  }
}

// SAFETY: ctx points to a live ReaderBridge owned by the ActiveStream. The
// codec serializes callback access and supplies a writable buffer of len bytes.
unsafe extern "C" fn read_cb(ctx: *mut c_void, buf: *mut u8, len: usize) -> i64 {
  if ctx.is_null() {
    return -1;
  }
  let bridge = &mut *(ctx as *mut ReaderBridge<Box<dyn ReadSeekSend>>);
  let slice = unsafe { std::slice::from_raw_parts_mut(buf, len) };
  match bridge.reader.read(slice) {
    Ok(n) => n.try_into().unwrap_or(-1),
    Err(_) => -1,
  }
}

// SAFETY: ctx points to the same live ReaderBridge as read_cb. The codec
// serializes read/seek calls for a stream.
unsafe extern "C" fn seek_cb(ctx: *mut c_void, pos: i64, whence: i32) -> i64 {
  if ctx.is_null() {
    return -1;
  }
  let bridge = &mut *(ctx as *mut ReaderBridge<Box<dyn ReadSeekSend>>);
  use std::io::SeekFrom;
  let from = match whence {
    0 => {
      let Ok(pos) = u64::try_from(pos) else {
        return -1;
      };
      SeekFrom::Start(pos)
    }
    1 => SeekFrom::Current(pos),
    2 => SeekFrom::End(pos),
    _ => return -1,
  };
  match bridge.reader.seek(from) {
    Ok(n) => i64::try_from(n).unwrap_or(-1),
    Err(_) => -1,
  }
}
type StreamResources = (
  StreamPtr,
  SafeCodecRef<'static>,
  ReaderPtr,
  Option<channel::Receiver<u64>>,
  u32,
  u8,
  String,
  Arc<StreamState>,
  CancellationToken,
);

fn read_codec_block(
  codec_ref: SafeCodecRef<'static>,
  stream_ptr: StreamPtr,
  buffer: &mut [f32],
) -> i32 {
  codec_ref.read(stream_ptr.0, buffer.as_mut_ptr(), buffer.len())
}

// ---------------------------------------------------------------------------
// Engine impl
// ---------------------------------------------------------------------------

impl Engine {
  pub fn new(config: crate::config::EngineConfig) -> Self {
    init_codec();
    around_audio_sdk::filter::init_filter();
    Self {
      config,
      device_lost: Arc::new(AtomicBool::new(false)),
      streams: RwLock::new(HashMap::new()),
      shutdown_token: CancellationToken::new(),
    }
  }

  /// Prepare a source for playback. Opens the codec, creates a stream,
  /// and returns metadata without starting the decode loop.
  pub fn prepare(&self, source: Box<dyn Source>) -> Result<PreparedStream, AroundError> {
    let seekable = source.capabilities().contains(SourceCapabilities::SEEKABLE);
    let (stream_ptr, info, codec_ref, reader_ctx) = Self::open_codec(source.as_ref(), seekable)?;
    let codec_name = {
      let p = codec_ref.name();
      // SAFETY: name() returns a valid null-terminated C string pointer.
      unsafe {
        std::ffi::CStr::from_ptr(p as *const i8)
          .to_string_lossy()
          .into_owned()
      }
    };

    let sample_rate = info.sample_rate;
    if sample_rate == 0 {
      codec_ref.drop(stream_ptr);
      drop_reader_context(reader_ctx);
      return Err(AroundError::DecodeError {
        message: "codec returned sample_rate=0".into(),
      });
    }
    let channels = info.channels;
    let total_frames = info.total_frames;

    let source_format = match SampleSpec::interleaved(sample_rate, channels, PcmEncoding::F32) {
      Ok(fmt) => fmt,
      Err(e) => {
        codec_ref.drop(stream_ptr);
        drop_reader_context(reader_ctx);
        return Err(AroundError::DecodeError {
          message: format!("invalid stream spec: {e}"),
        });
      }
    };

    let duration_ms = if total_frames > 0 {
      let duration = u128::from(total_frames) * 1000 / u128::from(sample_rate);
      Some(u64::try_from(duration).unwrap_or(u64::MAX))
    } else {
      None
    };

    let stream_id = next_stream_id();
    let source_path = source.identifier();
    let content_type = source.content_type();
    let stream_state = Arc::new(StreamState::new());
    stream_state.set_status(PlaybackStatus::Buffering);
    stream_state.active.store(true, Ordering::SeqCst);
    stream_state.position_ms.store(0, Ordering::SeqCst);

    let cancel = self.shutdown_token.child_token();
    let (seek_tx, seek_rx) = channel::unbounded();

    let active = ActiveStream {
      id: stream_id,
      state: Arc::clone(&stream_state),
      cancel: cancel.clone(),
      stream_ptr: Some(StreamPtr(stream_ptr)),
      codec_ref: Some(codec_ref),
      reader_ctx: Some(reader_ctx),
      sample_rate,
      channels,
      total_frames,
      content_type,
      source_path,
      codec_name: codec_name.clone(),
      duration_ms,
      seekable,
      source_format,
      output_spec: None,
      seek_tx: Some(seek_tx),
      seek_rx: Some(seek_rx),
    };
    self.streams.write().insert(stream_id, active);
    debug_track_stream(true);

    Ok(PreparedStream {
      stream_id,
      codec_name,
      duration_ms,
      seekable,
      source_format,
    })
  }

  pub fn run_stream(&self, id: StreamId) -> Result<(), AroundError> {
    let session = self.open_output_session(id)?;
    self.set_stream_output_spec(id, session.target_spec())?;
    self.run_stream_with_session(id, session)
  }
  fn run_stream_with_session(
    &self,
    id: StreamId,
    mut output_session: OutputSession,
  ) -> Result<(), AroundError> {
    // Take ownership of decode resources from the stream map.
    let (
      stream_ptr,
      codec_ref,
      reader_ctx,
      seek_rx,
      sample_rate,
      channels,
      codec_name,
      stream_state,
      cancel,
    ) = {
      let mut streams = self.streams.write();
      let s = streams.get_mut(&id).ok_or_else(|| AroundError::Internal {
        message: format!("run_stream: stream {id} not found"),
      })?;
      let StreamPtr(stream_ptr) = s.stream_ptr.take().ok_or_else(|| AroundError::Internal {
        message: format!("run_stream: stream {id} resources already consumed"),
      })?;
      let codec_ref = s.codec_ref.take().ok_or_else(|| AroundError::Internal {
        message: format!("run_stream: stream {id} codec already consumed"),
      })?;
      let reader_ctx = s.reader_ctx.take().ok_or_else(|| AroundError::Internal {
        message: format!("run_stream: stream {id} reader already consumed"),
      })?;
      let seek_rx = s.seek_rx.take();
      let codec_name = s.codec_name.clone();
      let stream_state = s.state.clone();
      let cancel = s.cancel.clone();
      (
        stream_ptr,
        codec_ref,
        reader_ctx,
        seek_rx,
        s.sample_rate,
        s.channels,
        codec_name,
        stream_state,
        cancel,
      )
    };
    let max_samples = 4096 * usize::from(channels.max(1));
    let (processing_samples, output_bytes) = output_session.workspace_requirements(max_samples);
    let mut pcm_buf = crate::pcm_buffer::PcmBuffer::with_processing_capacity(
      max_samples,
      processing_samples,
      output_bytes,
    );
    let _span = tracing::info_span!("playback", id).entered();
    tracing::info!(id, codec = codec_name, "starting playback");
    stream_state.set_status(PlaybackStatus::Playing);

    let mut total_frames_decoded: u64 = 0;
    let mut consecutive_errors = 0u32;
    let mut decode_error: Option<AroundError> = None;

    loop {
      if cancel.is_cancelled() {
        tracing::debug!(id, "stream cancelled");
        break;
      }

      if self.device_lost.swap(false, Ordering::SeqCst) {
        tracing::warn!(id, "audio device lost");
        stream_state.device_lost.store(true, Ordering::SeqCst);
        if self.config.output_auto_reconnect {
          if let Err(error) = output_session.rebind() {
            stream_state.set_status(PlaybackStatus::Error);
            decode_error = Some(AroundError::Internal {
              message: format!("output rebind failed: {error}"),
            });
            break;
          }
          stream_state.device_lost.store(false, Ordering::SeqCst);
          let (processing_samples, output_bytes) =
            output_session.workspace_requirements(max_samples);
          pcm_buf = crate::pcm_buffer::PcmBuffer::with_processing_capacity(
            max_samples,
            processing_samples,
            output_bytes,
          );
        } else {
          stream_state.set_status(PlaybackStatus::Error);
          break;
        }
      }

      if stream_state.status() == PlaybackStatus::Paused {
        std::thread::sleep(std::time::Duration::from_millis(20));
        continue;
      }

      if let Some(rx) = &seek_rx {
        while let Ok(position_ms) = rx.try_recv() {
          let requested = u128::from(position_ms) * u128::from(sample_rate) / 1000;
          let frame = u64::try_from(requested).unwrap_or(u64::MAX);
          match codec_ref.seek(stream_ptr, frame) {
            new_pos if new_pos >= 0 => {
              total_frames_decoded = new_pos.unsigned_abs();
              let pos = total_frames_decoded * 1000 / u64::from(sample_rate);
              stream_state.position_ms.store(pos, Ordering::Relaxed);
              stream_state.set_status(PlaybackStatus::Playing);
            }
            _ => tracing::warn!(id, position_ms, "seek returned error"),
          }
        }
      }

      let n = {
        let buf = pcm_buf.decode_region();
        codec_ref.read(stream_ptr, buf.as_mut_ptr(), buf.len())
      };
      if n > 0 {
        let sample_count = usize::try_from(n).unwrap_or(0);
        let frames = sample_count / usize::from(channels.max(1));
        if let Err(error) = output_session.submit_workspace(&mut pcm_buf, frames) {
          if matches!(error, crate::output_session::OutputError::Lost)
            && self.config.output_auto_reconnect
          {
            stream_state.set_status(PlaybackStatus::Buffering);
            if let Err(rebind_error) = output_session.rebind() {
              stream_state.set_status(PlaybackStatus::Error);
              decode_error = Some(AroundError::Internal {
                message: format!("output rebind failed: {rebind_error}"),
              });
              break;
            }
            let (processing_samples, output_bytes) =
              output_session.workspace_requirements(max_samples);
            pcm_buf = crate::pcm_buffer::PcmBuffer::with_processing_capacity(
              max_samples,
              processing_samples,
              output_bytes,
            );
            stream_state.set_status(PlaybackStatus::Playing);
            continue;
          }
          tracing::error!(id, error = ?error, "output write error");
          stream_state.set_status(PlaybackStatus::Error);
          decode_error = Some(AroundError::Internal {
            message: error.to_string(),
          });
          break;
        }
        total_frames_decoded += u64::try_from(frames).unwrap_or(0);
        consecutive_errors = 0;
        let pos = total_frames_decoded * 1000 / u64::from(sample_rate);
        stream_state.position_ms.store(pos, Ordering::Relaxed);
      } else if n == 0 {
        break;
      } else {
        consecutive_errors += 1;
        tracing::warn!(id, n, consecutive_errors, "decode error");
        if consecutive_errors >= 3 {
          stream_state.set_status(PlaybackStatus::Error);
          decode_error = Some(AroundError::DecodeError {
            message: format!("decode error: code {n}"),
          });
          break;
        }
      }
    }

    tracing::info!(id, total_frames = total_frames_decoded, "playback complete");
    // Only overwrite Error status with Stopped if we exited normally
    // (EOF, cancel, device-lost) — not on decode error.
    if decode_error.is_none() {
      stream_state.set_status(PlaybackStatus::Stopped);
    }
    stream_state.active.store(false, Ordering::SeqCst);
    codec_ref.drop(stream_ptr);
    drop_reader_context(reader_ctx);
    debug_track_stream(false);

    // Cleanup stream from engine.
    self.streams.write().remove(&id);

    if let Some(err) = decode_error {
      Err(err)
    } else {
      Ok(())
    }
  }

  #[cfg(feature = "async-decode")]
  /// Run a Stream through an OutputSession without moving orchestration to a
  /// blocking thread. Codec reads stay on the blocking pool; output binding
  /// receives exact bytes from FilterChain.
  pub async fn run_stream_async(&self, id: StreamId) -> Result<(), AroundError> {
    let mut output_session = self.open_output_session(id)?;
    self.set_stream_output_spec(id, output_session.target_spec())?;
    let (
      stream_ptr,
      codec_ref,
      reader_ctx,
      seek_rx,
      sample_rate,
      channels,
      codec_name,
      stream_state,
      cancel,
    ) = self.take_stream_resources(id, "run_stream_async")?;

    let _span = tracing::info_span!("playback", id);
    tracing::info!(id, codec = codec_name, "starting async playback");
    stream_state.set_status(PlaybackStatus::Playing);

    let max_samples = 4096 * usize::from(channels.max(1));
    let (processing_samples, output_bytes) = output_session.workspace_requirements(max_samples);
    let mut pcm_buf = crate::pcm_buffer::PcmBuffer::with_processing_capacity(
      max_samples,
      processing_samples,
      output_bytes,
    );
    let mut total_frames_decoded: u64 = 0;
    let mut consecutive_errors = 0u32;
    let mut decode_error: Option<AroundError> = None;

    loop {
      if cancel.is_cancelled() {
        break;
      }
      if self.device_lost.swap(false, Ordering::SeqCst) {
        stream_state.device_lost.store(true, Ordering::SeqCst);
        if self.config.output_auto_reconnect {
          if let Err(error) = output_session.rebind() {
            stream_state.set_status(PlaybackStatus::Error);
            decode_error = Some(AroundError::Internal {
              message: format!("output rebind failed: {error}"),
            });
            break;
          }
          let (processing_samples, output_bytes) =
            output_session.workspace_requirements(max_samples);
          pcm_buf = crate::pcm_buffer::PcmBuffer::with_processing_capacity(
            max_samples,
            processing_samples,
            output_bytes,
          );
          stream_state.device_lost.store(false, Ordering::SeqCst);
        } else {
          stream_state.set_status(PlaybackStatus::Error);
          break;
        }
      }
      if stream_state.status() == PlaybackStatus::Paused {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        continue;
      }
      if let Some(rx) = &seek_rx {
        while let Ok(position_ms) = rx.try_recv() {
          let requested = u128::from(position_ms) * u128::from(sample_rate) / 1000;
          let frame = u64::try_from(requested).unwrap_or(u64::MAX);
          if codec_ref.seek(stream_ptr.0, frame) >= 0 {
            total_frames_decoded = frame;
            stream_state.position_ms.store(
              total_frames_decoded * 1000 / u64::from(sample_rate),
              Ordering::Relaxed,
            );
            stream_state.set_status(PlaybackStatus::Playing);
          }
        }
      }

      let local_buf = std::mem::take(&mut pcm_buf);
      let stream_addr = stream_ptr.0 as usize;
      let decode_result = tokio::task::spawn_blocking(move || {
        let mut local_buf = local_buf;
        let slice = local_buf.decode_region();
        let stream_ptr = StreamPtr(stream_addr as *mut c_void);
        let n = read_codec_block(codec_ref, stream_ptr, slice);
        (n, local_buf)
      })
      .await;
      let (n, returned_buf) = match decode_result {
        Ok(result) => result,
        Err(join) => {
          stream_state.set_status(PlaybackStatus::Error);
          decode_error = Some(AroundError::Internal {
            message: format!("decode thread panicked: {join}"),
          });
          break;
        }
      };
      pcm_buf = returned_buf;

      if n > 0 {
        let sample_count = usize::try_from(n).unwrap_or(0);
        let frames = sample_count / usize::from(channels.max(1));
        if let Err(error) = output_session.submit_workspace(&mut pcm_buf, frames) {
          if matches!(error, crate::output_session::OutputError::Lost)
            && self.config.output_auto_reconnect
          {
            stream_state.set_status(PlaybackStatus::Buffering);
            if let Err(rebind_error) = output_session.rebind() {
              stream_state.set_status(PlaybackStatus::Error);
              decode_error = Some(AroundError::Internal {
                message: format!("output rebind failed: {rebind_error}"),
              });
              break;
            }
            let (processing_samples, output_bytes) =
              output_session.workspace_requirements(max_samples);
            pcm_buf = crate::pcm_buffer::PcmBuffer::with_processing_capacity(
              max_samples,
              processing_samples,
              output_bytes,
            );
            stream_state.set_status(PlaybackStatus::Playing);
            continue;
          }
          stream_state.set_status(PlaybackStatus::Error);
          decode_error = Some(AroundError::Internal {
            message: error.to_string(),
          });
          break;
        }
        total_frames_decoded += u64::try_from(frames).unwrap_or(0);
        consecutive_errors = 0;
        stream_state.position_ms.store(
          total_frames_decoded * 1000 / u64::from(sample_rate),
          Ordering::Relaxed,
        );
      } else if n == 0 {
        break;
      } else {
        consecutive_errors += 1;
        if consecutive_errors >= 3 {
          stream_state.set_status(PlaybackStatus::Error);
          decode_error = Some(AroundError::DecodeError {
            message: format!("decode error: code {n}"),
          });
          break;
        }
      }
    }

    if decode_error.is_none() {
      stream_state.set_status(PlaybackStatus::Stopped);
    }
    stream_state.active.store(false, Ordering::SeqCst);
    codec_ref.drop(stream_ptr.0);
    drop_reader_context(reader_ctx);
    debug_track_stream(false);
    self.streams.write().remove(&id);
    decode_error.map_or(Ok(()), Err)
  }

  #[cfg(feature = "async-decode")]
  pub fn run_stream_spawn(self: &Arc<Self>, id: StreamId) -> Result<(), AroundError> {
    let this = self.clone();
    tokio::spawn(async move {
      if let Err(error) = this.run_stream_async(id).await {
        tracing::error!(?error, "async playback error");
      }
    });
    Ok(())
  }
  /// Seek a running stream to an absolute position in milliseconds.
  /// Returns the requested position on success, or a SeekError.
  ///
  /// The seek is applied asynchronously via a channel to the decode loop.
  /// The returned Ok acknowledges the request was queued; the actual seek
  /// may complete one decode iteration later (best-effort).
  pub fn seek_stream(&self, id: StreamId, position_ms: u64) -> Result<u64, SeekError> {
    let streams = self.streams.read();
    let s = streams.get(&id).ok_or(SeekError::StreamNotFound)?;
    if !s.seekable {
      return Err(SeekError::NotSeekable);
    }
    // Validate position against known duration if available.
    if let Some(dur) = s.duration_ms {
      if position_ms > dur {
        return Err(SeekError::StreamEnded);
      }
    }
    let st = s.state.status();
    if st == PlaybackStatus::Stopped || st == PlaybackStatus::Error {
      return Err(SeekError::StreamEnded);
    }
    if let Some(tx) = &s.seek_tx {
      tx.send(position_ms).map_err(|_| SeekError::StreamEnded)?;
      s.state.set_status(PlaybackStatus::Buffering);
      Ok(position_ms)
    } else {
      Err(SeekError::StreamEnded)
    }
  }

  /// Stop a specific stream by ID, or all streams if id is None.
  pub fn stop_stream(&self, id: Option<StreamId>) {
    let streams = self.streams.read();
    if let Some(id) = id {
      if let Some(s) = streams.get(&id) {
        s.cancel.cancel();
        s.state.set_status(PlaybackStatus::Stopped);
      }
    } else {
      for s in streams.values() {
        s.cancel.cancel();
        s.state.set_status(PlaybackStatus::Stopped);
      }
    }
  }

  /// Pause a specific stream.
  pub fn pause_stream(&self, id: StreamId) {
    let streams = self.streams.read();
    if let Some(s) = streams.get(&id) {
      s.state.set_status(PlaybackStatus::Paused);
      tracing::debug!(id, "stream paused");
    }
  }

  /// Resume a specific stream.
  pub fn resume_stream(&self, id: StreamId) {
    let streams = self.streams.read();
    if let Some(s) = streams.get(&id) {
      s.state.set_status(PlaybackStatus::Playing);
      tracing::debug!(id, "stream resumed");
    }
  }

  /// Get state for a specific stream.
  pub fn stream_state(&self, id: StreamId) -> Option<Arc<StreamState>> {
    self.streams.read().get(&id).map(|s| Arc::clone(&s.state))
  }

  /// List all stream IDs.
  pub fn stream_ids(&self) -> Vec<StreamId> {
    self.streams.read().keys().copied().collect()
  }

  /// Get the source path for a stream.
  pub fn stream_source_path(&self, id: StreamId) -> Option<String> {
    self.streams.read().get(&id).map(|s| s.source_path.clone())
  }

  /// Get the codec name for a stream.
  pub fn stream_codec_name(&self, id: StreamId) -> Option<String> {
    self.streams.read().get(&id).map(|s| s.codec_name.clone())
  }

  /// Get the duration in milliseconds for a stream.
  pub fn stream_duration_ms(&self, id: StreamId) -> Option<u64> {
    self.streams.read().get(&id).and_then(|s| s.duration_ms)
  }

  /// Get the content type for a stream.
  pub fn stream_content_type(&self, id: StreamId) -> Option<String> {
    self
      .streams
      .read()
      .get(&id)
      .and_then(|s| s.content_type.clone())
  }

  /// Get the seekable flag for a stream.
  pub fn stream_seekable(&self, id: StreamId) -> Option<bool> {
    self.streams.read().get(&id).map(|s| s.seekable)
  }
  /// Get the exact native output format selected by OutputSession.
  pub fn stream_output_spec(&self, id: StreamId) -> Option<SampleSpec> {
    self.streams.read().get(&id).and_then(|s| s.output_spec)
  }

  /// Returns the sole active stream ID, or None.
  pub fn sole_stream_id(&self) -> Option<StreamId> {
    let streams = self.streams.read();
    let active: Vec<StreamId> = streams
      .values()
      .filter(|s| s.state.active.load(Ordering::SeqCst))
      .map(|s| s.id)
      .collect();
    if active.len() == 1 {
      Some(active[0])
    } else {
      None
    }
  }

  /// Shutdown all streams and the engine.
  pub fn shutdown(&self) {
    self.shutdown_token.cancel();
    for s in self.streams.read().values() {
      s.cancel.cancel();
    }
  }

  pub fn is_shutdown(&self) -> bool {
    self.shutdown_token.is_cancelled()
  }

  // Legacy methods for backward compat with single-stream IPC.
  pub fn stop(&self) {
    self.stop_stream(None);
  }

  pub fn pause(&self) {
    if let Some(id) = self.sole_stream_id() {
      self.pause_stream(id);
    }
  }

  pub fn resume(&self) {
    if let Some(id) = self.sole_stream_id() {
      self.resume_stream(id);
    }
  }

  pub fn seek(&self, _position_ms: u64) {
    // Seek not yet implemented for multi-stream.
  }

  pub fn running(&self) -> bool {
    self
      .streams
      .read()
      .values()
      .any(|s| s.state.active.load(Ordering::SeqCst))
  }

  pub fn config(&self) -> &crate::config::EngineConfig {
    &self.config
  }

  pub fn notify_device_lost(&self) {
    self.device_lost.store(true, Ordering::SeqCst);
    tracing::warn!("audio output device disconnected");
    if !self.config.output_auto_reconnect {
      self.stop();
    }
  }

  fn set_stream_output_spec(
    &self,
    id: StreamId,
    output_spec: SampleSpec,
  ) -> Result<(), AroundError> {
    let mut streams = self.streams.write();
    let stream = streams.get_mut(&id).ok_or_else(|| AroundError::Internal {
      message: format!("output session: stream {id} not found"),
    })?;
    stream.output_spec = Some(output_spec);
    Ok(())
  }
  pub fn is_device_lost(&self) -> bool {
    self.device_lost.load(Ordering::SeqCst)
  }

  // -----------------------------------------------------------------------
  // Codec opening (internal)
  // -----------------------------------------------------------------------

  fn open_output_session(&self, id: StreamId) -> Result<OutputSession, AroundError> {
    let source = {
      let streams = self.streams.read();
      let stream = streams.get(&id).ok_or_else(|| AroundError::Internal {
        message: format!("output session: stream {id} not found"),
      })?;
      stream.source_format
    };
    let recovery = if self.config.output_auto_reconnect {
      RecoveryPolicy::ContinueOnFallback
    } else {
      RecoveryPolicy::Stop
    };
    let selector = self
      .config
      .output_device
      .clone()
      .map_or(DeviceSelector::Any, |value| {
        if value.starts_with("cpal:") {
          DeviceSelector::Exact(value)
        } else {
          DeviceSelector::Name(value)
        }
      });
    let preferences = vec![OutputPreference {
      selector,
      mode: ModePreference::Any,
      recovery,
    }];
    let (discovery, binder): (
      Box<dyn crate::output_session::OutputDiscovery>,
      Box<dyn crate::output_session::OutputBinder>,
    ) = match self.config.output_driver {
      OutputDriver::Null => {
        let discovery = null_adapter(source);
        let binder = discovery.clone();
        (Box::new(discovery), Box::new(binder))
      }
      OutputDriver::Cpal => {
        let adapter = crate::cpal_adapter::CpalOutputAdapter::new(0);
        let binder = adapter.clone();
        (Box::new(adapter), Box::new(binder))
      }
    };
    OutputSession::open(discovery, binder, source, vec![], preferences).map_err(|error| {
      AroundError::Internal {
        message: format!("open output session failed: {error}"),
      }
    })
  }

  fn take_stream_resources(
    &self,
    id: StreamId,
    operation: &str,
  ) -> Result<StreamResources, AroundError> {
    let mut streams = self.streams.write();
    let stream = streams.get_mut(&id).ok_or_else(|| AroundError::Internal {
      message: format!("{operation}: stream {id} not found"),
    })?;
    let stream_ptr = stream
      .stream_ptr
      .take()
      .ok_or_else(|| AroundError::Internal {
        message: format!("{operation}: stream resources already consumed"),
      })?;
    let codec_ref = stream
      .codec_ref
      .take()
      .ok_or_else(|| AroundError::Internal {
        message: format!("{operation}: codec already consumed"),
      })?;
    let reader_ctx = stream
      .reader_ctx
      .take()
      .ok_or_else(|| AroundError::Internal {
        message: format!("{operation}: reader already consumed"),
      })?;
    Ok((
      stream_ptr,
      codec_ref,
      reader_ctx,
      stream.seek_rx.take(),
      stream.sample_rate,
      stream.channels,
      stream.codec_name.clone(),
      Arc::clone(&stream.state),
      stream.cancel.clone(),
    ))
  }

  fn open_codec(
    source: &dyn Source,
    _seekable: bool,
  ) -> Result<(*mut c_void, StreamInfo, SafeCodecRef<'static>, ReaderPtr), AroundError> {
    let register = init_codec();

    // --- Phase 1: Read header bytes for probing (separate reader) ---
    let header_buf = {
      #[expect(
        deprecated,
        reason = "Phase 1 backward compat: open_seekable() is deprecated in favor of async read()/seek(); will be migrated in Phase 2 codec refactor"
      )]
      let probe_reader = source.open_seekable()?;
      let probe_ctx = Box::into_raw(Box::new(ReaderBridge {
        reader: probe_reader,
      })) as *mut c_void;
      let mut buf = [0u8; 8192];
      // SAFETY: probe_ctx was allocated via Box::into_raw(Box::new(probe_ctx)) on the preceding line. probe_buf is a valid stack-allocated buffer. These are scoped to the probe() call.
      let n = unsafe { read_cb(probe_ctx, buf.as_mut_ptr(), buf.len()) };
      if !probe_ctx.is_null() {
        unsafe {
          drop(Box::from_raw(
            probe_ctx as *mut ReaderBridge<Box<dyn ReadSeekSend>>,
          ));
        }
      }
      let len = usize::try_from(n).unwrap_or(0);
      buf[..len].to_vec()
    };

    let header = &header_buf[..];
    let filename = b"";

    // --- Phase 2: Probe through safe codec entries ---
    let mut best_conf = 0u8;
    let mut best_codec: Option<SafeCodecRef<'static>> = None;

    register.for_each_codec(|codec: SafeCodecRef<'_>| {
      let conf = codec.probe(
        header.as_ptr(),
        header.len(),
        filename.as_ptr(),
        filename.len(),
      );
      if conf >= 50 && conf > best_conf {
        best_conf = conf;
        // SAFETY: This codec entry will not be removed during the
        // engine's lifetime.  The engine holds no concurrent remove_by_meta
        // call, and Framework::unload() only runs after engine shutdown.
        // This is the same contract as the existing design prelude Decision 2.
        best_codec = Some(unsafe { codec.assume_static() });
      }
    });

    // --- Phase 3: Choose codec and open with a fresh reader ---
    let chosen: SafeCodecRef<'static> = best_codec.unwrap_or_else(|| {
      // Built-in fallback: leak a WavCodec DynCodecRef
      SafeCodecRef::from_owned(around_codec_wav::WavCodec)
    });

    #[expect(
      deprecated,
      reason = "Phase 1 backward compat: open_seekable() is deprecated in favor of async read()/seek(); will be migrated in Phase 2 codec refactor"
    )]
    let reader = source.open_seekable()?;
    let reader_ctx = Box::into_raw(Box::new(ReaderBridge { reader })) as *mut c_void;

    let mut stream: *mut c_void = std::ptr::null_mut();
    // SAFETY: StreamInfo is a repr(C) struct of primitive fields. All-zero is a valid bit pattern — the codec's open() method will overwrite the fields with actual metadata before returning.
    let mut info: StreamInfo = unsafe { std::mem::zeroed() };
    let rc = chosen.open(reader_ctx, read_cb, seek_cb, &mut stream, &mut info);

    if rc == 0 && !stream.is_null() {
      // The codec retains reader_ctx for read/seek callbacks. ActiveStream
      // owns it and drops it only after chosen.drop(stream).
      Ok((stream, info, chosen, ReaderPtr(reader_ctx)))
    } else {
      drop_reader_context(ReaderPtr(reader_ctx));
      Err(AroundError::UnsupportedFormat {
        format: Some("unknown".into()),
        reason: "no codec could open the source".into(),
      })
    }
  }
}

impl Drop for Engine {
  fn drop(&mut self) {
    self.shutdown();
  }
}

#[cfg(all(test, feature = "async-decode"))]
mod tests {
  #![expect(
    clippy::expect_used,
    reason = "committed audio fixture and freshly prepared stream are test invariants"
  )]
  use super::*;

  #[test]
  fn sync_decode_keeps_reader_alive_to_eof() {
    let mut config = crate::config::EngineConfig::load();
    config.output_driver = OutputDriver::Null;
    let engine = Engine::new(config);
    let fixture =
      std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/example.wav");
    let prepared = engine
      .prepare(Box::new(around_source_file::FileSource::new(fixture)))
      .expect("fixture must prepare");
    let state = engine
      .stream_state(prepared.stream_id)
      .expect("prepared stream must have state");
    assert_eq!(
      engine.stream_content_type(prepared.stream_id).as_deref(),
      Some("audio/wav")
    );
    let planned = engine
      .open_output_session(prepared.stream_id)
      .expect("null output session must plan");
    assert_eq!(planned.target_spec().encoding, PcmEncoding::F32);
    assert_eq!(
      planned.target_spec().sample_rate,
      prepared.source_format.sample_rate
    );

    engine
      .run_stream(prepared.stream_id)
      .expect("sync decode must reach EOF");

    assert_eq!(state.status(), PlaybackStatus::Stopped);
    assert!(!state.active.load(Ordering::SeqCst));
    assert!(engine.stream_ids().is_empty());
  }

  #[tokio::test]
  async fn async_decode_consumes_file_source_to_eof() {
    let mut config = crate::config::EngineConfig::load();
    config.output_driver = OutputDriver::Null;
    let engine = Engine::new(config);
    let fixture =
      std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/example.wav");
    let prepared = engine
      .prepare(Box::new(around_source_file::FileSource::new(fixture)))
      .expect("fixture must prepare");
    let state = engine
      .stream_state(prepared.stream_id)
      .expect("prepared stream must have state");
    engine
      .run_stream_async(prepared.stream_id)
      .await
      .expect("async decode must reach EOF");

    assert_eq!(state.status(), PlaybackStatus::Stopped);
    assert!(!state.active.load(Ordering::SeqCst));
    assert!(engine.stream_ids().is_empty());
  }
}
