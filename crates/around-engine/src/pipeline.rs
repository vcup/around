//! Audio pipeline: multi-stream engine (ADR-0005 compliant).
//!
//! Single `Engine` manages 0..N concurrent `Stream`s. Each stream has
//! independent state via all-Atomic `StreamState` and `CancellationToken`.

use crate::filter_chain::FilterChain;
use crate::ipc::types::{PlaybackState, TrackState};
use crate::output::create_output;
use around_core::codec::{init_codec, CodecDyn, CodecRegister, DynCodecRef, StreamInfo};
use around_core::state::PlaybackStatus;
use around_core::{AroundError, SampleSpec, Source, SourceCapabilities};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Read;
use std::mem::ManuallyDrop;
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

pub struct StreamState {
  status: AtomicU8,
  pub position_ms: AtomicU64,
  pub device_lost: AtomicBool,
  pub active: AtomicBool,
}

impl StreamState {
  pub fn new() -> Self {
    Self {
      status: AtomicU8::new(PlaybackStatus::Stopped as u8),
      position_ms: AtomicU64::new(0),
      device_lost: AtomicBool::new(false),
      active: AtomicBool::new(false),
    }
  }

  pub fn status(&self) -> PlaybackStatus {
    let raw = self.status.load(Ordering::SeqCst);
    match raw {
      0 => PlaybackStatus::Playing,
      1 => PlaybackStatus::Paused,
      2 => PlaybackStatus::Stopped,
      3 => PlaybackStatus::Buffering,
      _ => PlaybackStatus::Error,
    }
  }

  pub fn set_status(&self, s: PlaybackStatus) {
    self.status.store(s as u8, Ordering::SeqCst);
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

struct ActiveStream {
  id: StreamId,
  state: Arc<StreamState>,
  cancel: CancellationToken,
}

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
  pub output_format: SampleSpec,
  pub stream_id: StreamId,
}

impl PlaybackHandle {
  pub fn stream_id(&self) -> StreamId {
    self.stream_id
  }
}

// ---------------------------------------------------------------------------
// Reader callback helpers
// ---------------------------------------------------------------------------

trait ReadSend: Read + Send {}
impl<T: Read + Send> ReadSend for T {}

struct ReaderBridge<R: Read + Send> {
  reader: R,
}

unsafe extern "C" fn read_cb(ctx: *mut c_void, buf: *mut u8, len: usize) -> i64 {
  let bridge = &mut *(ctx as *mut ReaderBridge<Box<dyn ReadSend>>);
  let buf_slice = unsafe { std::slice::from_raw_parts_mut(buf, len) };
  match bridge.reader.read(buf_slice) {
    Ok(n) => n as i64,
    Err(_) => -1,
  }
}

unsafe extern "C" fn seek_cb(_ctx: *mut c_void, _pos: i64, _whence: i32) -> i64 {
  -1
}

// ---------------------------------------------------------------------------
// CodecHandle — dynamic dispatch through CodecRegister
// ---------------------------------------------------------------------------

/// Handle to a registered codec. Holds raw vtable + codec data pointers.
/// Methods dispatch through the vtable directly, avoiding ownership issues.
struct CodecHandle {
  vtable: *const (),
  codec_data: *mut (),
}

impl CodecHandle {
  /// Call `Codec::read` through the vtable.
  unsafe fn read(&self, stream: *mut c_void, buf: *mut f32, buf_len: usize) -> i32 {
    // VTable layout (stabby #[repr(C)]): methods in declaration order.
    // read is the 4th method (after probe, name, open).
    type ReadMethod = unsafe extern "C" fn(*const (), *mut c_void, *mut f32, usize) -> i32;
    let vtable = self.vtable as *const ReadMethod;
    let read_fn = unsafe { *vtable.add(3) };
    unsafe { read_fn(self.codec_data as *const (), stream, buf, buf_len) }
  }

  /// Call `Codec::drop` through the vtable. Named `destroy` to avoid conflict
  /// with `Drop::drop`.
  unsafe fn destroy(&self, stream: *mut c_void) {
    // drop is the 6th method (after probe, name, open, read, seek).
    type DropMethod = unsafe extern "C" fn(*const (), *mut c_void);
    let vtable = self.vtable as *const DropMethod;
    let drop_fn = unsafe { *vtable.add(5) };
    unsafe { drop_fn(self.codec_data as *const (), stream) };
  }

  /// Call `Codec::name` through the vtable.
  unsafe fn name(&self) -> *const u8 {
    // name is the 2nd method (after probe).
    type NameMethod = unsafe extern "C" fn(*const ()) -> *const u8;
    let vtable = self.vtable as *const NameMethod;
    let name_fn = unsafe { *vtable.add(1) };
    unsafe { name_fn(self.codec_data as *const ()) }
  }

  /// Create a non-owning DynCodecRef view for calling trait methods
  /// through the CodecDyn trait (probe, open).
  fn as_ref(&self) -> ManuallyDrop<DynCodecRef> {
    unsafe { ManuallyDrop::new(CodecRegister::from_raw(self.vtable, self.codec_data)) }
  }
}

// ---------------------------------------------------------------------------
// Engine impl
// ---------------------------------------------------------------------------

impl Engine {
  pub fn new(config: crate::config::EngineConfig) -> Self {
    init_codec();
    Self {
      config,
      device_lost: Arc::new(AtomicBool::new(false)),
      streams: RwLock::new(HashMap::new()),
      shutdown_token: CancellationToken::new(),
    }
  }

  /// Play a source. Blocks until playback completes or stop is signaled.
  /// Handles pause/resume internally. Returns stream_id.
  pub fn play(
    &self,
    source: Box<dyn Source>,
    state: Arc<std::sync::Mutex<PlaybackState>>,
  ) -> Result<PlaybackHandle, AroundError> {
    let seekable = source.capabilities().contains(SourceCapabilities::SEEKABLE);
    let (stream_ptr, info, codec_handle) = Self::open_codec(source.as_ref(), seekable)?;
    let codec_name = unsafe {
      let p = codec_handle.name();
      std::ffi::CStr::from_ptr(p as *const i8)
        .to_string_lossy()
        .into_owned()
    };

    let sample_rate = info.sample_rate as u64;
    if sample_rate == 0 {
      unsafe { codec_handle.destroy(stream_ptr) };
      return Err(AroundError::DecodeError {
        message: "codec returned sample_rate=0".into(),
      });
    }
    let channels = info.channels;
    let total_frames = info.total_frames;

    let output_format = SampleSpec::interleaved(sample_rate as u32, channels, 16).map_err(|e| {
      AroundError::DecodeError {
        message: format!("invalid stream spec: {}", e),
      }
    })?;

    // Build filter chain (empty = passthrough; filters added per user config).
    let decode_spec =
      SampleSpec::interleaved(sample_rate as u32, channels, 16).unwrap_or(output_format);
    let mut filter_chain = FilterChain::build(decode_spec, vec![], output_format);

    let duration_ms = if total_frames > 0 {
      Some((total_frames as u128 * 1000 / sample_rate as u128) as u64)
    } else {
      None
    };

    let stream_id = next_stream_id();
    let stream_state = Arc::new(StreamState::new());
    stream_state.set_status(PlaybackStatus::Buffering);
    stream_state.active.store(true, Ordering::SeqCst);
    stream_state.position_ms.store(0, Ordering::SeqCst);

    let cancel = self.shutdown_token.child_token();

    // Store the stream.
    let active = ActiveStream {
      id: stream_id,
      state: Arc::clone(&stream_state),
      cancel: cancel.clone(),
    };
    self.streams.write().insert(stream_id, active);

    // Update shared IPC state.
    let epoch = {
      let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
      st.playing = true;
      st.position_ms = 0;
      st.output_format = Some(output_format);
      st.format_name = Some(codec_name.clone());
      st.seekable = seekable;
      st.duration_ms = duration_ms;
      st.state = TrackState::Buffering;
      st.epoch += 1;
      st.epoch
    };

    // --- Audio output setup (ringbuf) ---
    let (mut producer, consumer) = create_output(16384);
    let consumer = Arc::new(std::sync::Mutex::new(consumer));

    let host = cpal::default_host();
    let device = host
      .default_output_device()
      .ok_or_else(|| AroundError::Internal {
        message: "no audio output device found".into(),
      })?;
    let config = device
      .default_output_config()
      .map_err(|e| AroundError::Internal {
        message: format!("audio device error: {}", e),
      })?;

    let consumer_clone = Arc::clone(&consumer);
    let device_lost_flag = Arc::clone(&self.device_lost);
    let cpal_stream = device
      .build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
          if let Ok(mut cons) = consumer_clone.lock() {
            let popped = cons.pop_slice(data);
            if popped < data.len() {
              data[popped..].fill(0.0);
            }
          } else {
            data.fill(0.0);
          }
        },
        move |err| {
          tracing::error!(?err, "cpal stream error");
          device_lost_flag.store(true, Ordering::SeqCst);
        },
        None,
      )
      .map_err(|e| AroundError::Internal {
        message: format!("failed to build audio output stream: {}", e),
      })?;

    cpal_stream.play().map_err(|e| AroundError::Internal {
      message: format!("failed to start audio output stream: {}", e),
    })?;
    // --- Decode loop (blocking) ---
    let _span = tracing::info_span!("playback", stream_id).entered();
    tracing::info!(stream_id, codec = codec_name, "starting playback");

    {
      let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
      st.state = TrackState::Playing;
    }

    let max_ch = output_format.channels.max(1) as usize;
    let mut buf = vec![0.0f32; 4096 * max_ch];
    let mut total_frames: u64 = 0;
    let mut consecutive_errors = 0u32;

    stream_state.set_status(PlaybackStatus::Playing);

    loop {
      // Check cancellation.
      if cancel.is_cancelled() {
        tracing::debug!(stream_id, "stream cancelled");
        break;
      }

      if self.device_lost.load(Ordering::SeqCst) {
        tracing::warn!(stream_id, "audio device lost");
        stream_state.set_status(PlaybackStatus::Error);
        stream_state.device_lost.store(true, Ordering::SeqCst);
        {
          let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
          st.device_lost = true;
        }
        if self.config.output_auto_reconnect {
          self.device_lost.store(false, Ordering::SeqCst);
          continue;
        } else {
          break;
        }
      }

      // Check pause.
      if stream_state.status() == PlaybackStatus::Paused {
        std::thread::sleep(std::time::Duration::from_millis(20));
        continue;
      }

      let n = unsafe { codec_handle.read(stream_ptr, buf.as_mut_ptr(), buf.len()) };
      if n > 0 {
        let sample_count = n as usize;
        // Run through filter chain (currently passthrough; filters added per user config).
        let processed = filter_chain.process(&mut buf[..sample_count], channels);
        producer.push_slice(&buf[..processed]);
        total_frames += (n as u64) / channels as u64;
        consecutive_errors = 0;
        let pos = total_frames * 1000 / sample_rate;
        stream_state.position_ms.store(pos, Ordering::SeqCst);
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        st.position_ms = pos;
      } else if n == 0 {
        break; // EOF
      } else {
        consecutive_errors += 1;
        tracing::warn!(stream_id, n, consecutive_errors, "decode error");
        if consecutive_errors >= 3 {
          stream_state.set_status(PlaybackStatus::Error);
          {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            if st.epoch == epoch {
              st.state = TrackState::Error;
              st.playing = false;
            }
          }
          unsafe { codec_handle.destroy(stream_ptr) };
          return Err(AroundError::DecodeError {
            message: format!("decode error: code {}", n),
          });
        }
      }
    }

    tracing::info!(stream_id, total_frames, "playback complete");
    stream_state.set_status(PlaybackStatus::Stopped);
    stream_state.active.store(false, Ordering::SeqCst);
    unsafe { codec_handle.destroy(stream_ptr) };

    {
      let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
      if st.epoch == epoch {
        st.state = TrackState::Stopped;
        st.playing = false;
        st.position_ms = total_frames * 1000 / sample_rate;
      }
    }

    // Cleanup stream from engine.
    self.streams.write().remove(&stream_id);

    Ok(PlaybackHandle {
      output_format,
      stream_id,
    })
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

  pub fn is_device_lost(&self) -> bool {
    self.device_lost.load(Ordering::SeqCst)
  }

  // -----------------------------------------------------------------------
  // Codec opening (internal)
  // -----------------------------------------------------------------------

  fn open_codec(
    source: &dyn Source,
    _seekable: bool,
  ) -> Result<(*mut c_void, StreamInfo, CodecHandle), AroundError> {
    let register = init_codec();

    // --- Phase 1: Read header bytes for probing (separate reader) ---
    let header_buf = {
      let probe_reader = source.open_seekable()?;
      let probe_reader: Box<dyn ReadSend> = Box::new(probe_reader);
      let probe_ctx = Box::into_raw(Box::new(ReaderBridge {
        reader: probe_reader,
      })) as *mut c_void;
      let mut buf = [0u8; 8192];
      let n = unsafe { read_cb(probe_ctx, buf.as_mut_ptr(), buf.len()) };
      if !probe_ctx.is_null() {
        unsafe {
          drop(Box::from_raw(
            probe_ctx as *mut ReaderBridge<Box<dyn ReadSend>>,
          ));
        }
      }
      let len = if n > 0 { n as usize } else { 0 };
      buf[..len].to_vec()
    };

    let header = &header_buf[..];
    let filename = b"";

    // --- Phase 2: Probe through CodecRegister ---
    let mut best_conf = 0u8;
    let mut best_vtable: *const () = std::ptr::null();
    let mut best_codec_data: *mut () = std::ptr::null_mut();

    register.for_each_entry(|vtable_ptr, entry_ptr| {
      // SAFETY: for_each_entry yields (vtable, entry) pairs from the register.
      // The entry is a heap-allocated DynCodecRef. At offset 0 is the codec data
      // pointer (the Box<()> containing the codec instance).
      let codec_data = unsafe { *(entry_ptr as *mut *mut ()) };
      let probe_ref = unsafe { ManuallyDrop::new(CodecRegister::from_raw(vtable_ptr, codec_data)) };
      let conf = probe_ref.probe(
        header.as_ptr(),
        header.len(),
        filename.as_ptr(),
        filename.len(),
      );
      if conf >= 50 && conf > best_conf {
        best_conf = conf;
        best_vtable = vtable_ptr;
        best_codec_data = codec_data;
      }
    });

    // --- Phase 3: Choose codec and open with a fresh reader ---
    let chosen_handle = if !best_vtable.is_null() {
      CodecHandle {
        vtable: best_vtable,
        codec_data: best_codec_data,
      }
    } else {
      // Built-in fallback: create temporary DynCodecRef for WavCodec
      let temp: ManuallyDrop<DynCodecRef> = ManuallyDrop::new(DynCodecRef::from(
        stabby::boxed::Box::new(around_codec_wav::WavCodec),
      ));
      // SAFETY: ManuallyDrop<DynCodecRef> is repr(transparent); reading offset 0
      // gives the inner codec data pointer, offset size_of::<*mut ()>() gives the vtable.
      let temp_inner: &DynCodecRef = &temp;
      let cd = unsafe { *(temp_inner as *const DynCodecRef as *const *mut ()) };
      let vt = unsafe {
        *((temp_inner as *const DynCodecRef as *const u8).add(std::mem::size_of::<*mut ()>())
          as *const *const ())
      };
      // temp is dropped here — ManuallyDrop prevents double-free
      CodecHandle {
        vtable: vt,
        codec_data: cd,
      }
    };

    let reader = source.open_seekable()?;
    let reader: Box<dyn ReadSend> = Box::new(reader);
    let reader_ctx = Box::into_raw(Box::new(ReaderBridge { reader })) as *mut c_void;

    let mut stream: *mut c_void = std::ptr::null_mut();
    let mut info: StreamInfo = unsafe { std::mem::zeroed() };
    let rc = {
      let codec_ref = chosen_handle.as_ref();
      codec_ref.open(reader_ctx, read_cb, seek_cb, &mut stream, &mut info)
    };

    if !reader_ctx.is_null() {
      unsafe {
        drop(Box::from_raw(
          reader_ctx as *mut ReaderBridge<Box<dyn ReadSend>>,
        ));
      }
    }

    if rc == 0 && !stream.is_null() {
      Ok((stream, info, chosen_handle))
    } else {
      Err(AroundError::UnsupportedFormat {
        format: Some("unknown".into()),
        reason: "no codec could open the source".into(),
      })
    }
  }
}
