//! around-codec-test-pcm: Minimal PCM codec for extension contract testing.
//!
//! Handles raw PCM data with a simple header format:
//! - 4 bytes: sample rate (u32 LE)
//! - 1 byte: channels (u8)
//! - 1 byte: bits per sample (u8)
//! - Remainder: raw PCM samples (interleaved if multi-channel)

use around_core::{
  AroundError, AudioStream, AudioStreamVTable, CodecInfo, FormatSignature, ReadSeek,
};
use std::io::Read;

// ---------------------------------------------------------------------------
// Format declarations
// ---------------------------------------------------------------------------

static PCM_FORMATS: &[FormatSignature] = &[FormatSignature::from_extension(
  "pcm",
  "Raw PCM with header",
)];

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct PcmState {
  samples: Vec<f32>,
  channels: u8,
  position: usize,
}

impl PcmState {
  fn open(mut reader: Box<dyn ReadSeek + Send>) -> Result<(Self, u32), AroundError> {
    let mut raw = Vec::new();
    reader
      .read_to_end(&mut raw)
      .map_err(|e| AroundError::DecodeError {
        message: format!("failed to read source: {}", e),
      })?;

    if raw.len() < 6 {
      return Err(AroundError::DecodeError {
        message: "file too small for PCM header".into(),
      });
    }

    let sample_rate = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let channels = raw[4];
    let bits_per_sample = raw[5];

    let pcm_data = &raw[6..];

    let samples: Vec<f32> = match bits_per_sample {
      16 => {
        let num_samples = pcm_data.len() / 2;
        let mut out = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
          let sample = i16::from_le_bytes([pcm_data[i * 2], pcm_data[i * 2 + 1]]);
          out.push(sample as f32 / 32768.0);
        }
        out
      }
      32 => {
        let num_samples = pcm_data.len() / 4;
        let mut out = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
          let sample = i32::from_le_bytes([
            pcm_data[i * 4],
            pcm_data[i * 4 + 1],
            pcm_data[i * 4 + 2],
            pcm_data[i * 4 + 3],
          ]);
          out.push(sample as f32 / 2147483648.0);
        }
        out
      }
      _ => {
        return Err(AroundError::DecodeError {
          message: format!("unsupported bits per sample: {}", bits_per_sample),
        });
      }
    };

    Ok((
      Self {
        samples,
        channels,
        position: 0,
      },
      sample_rate,
    ))
  }

  fn read_impl(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError> {
    if self.position >= self.samples.len() {
      return Ok(None);
    }
    let remaining = self.samples.len() - self.position;
    let to_copy = remaining.min(buf.len());
    buf[..to_copy].copy_from_slice(&self.samples[self.position..self.position + to_copy]);
    self.position += to_copy;
    Ok(Some(to_copy / self.channels as usize))
  }

  fn seek_impl(&mut self, offset: u64) -> Result<(), AroundError> {
    let ch = self.channels as u64;
    let sample_offset = offset * ch;
    if sample_offset > self.samples.len() as u64 {
      return Err(AroundError::InvalidPosition {
        position_ms: 0,
        duration_ms: None,
      });
    }
    self.position = sample_offset as usize;
    Ok(())
  }
}

// ---------------------------------------------------------------------------
// VTable wrappers
// ---------------------------------------------------------------------------

unsafe fn pcm_read(
  data: *mut std::ffi::c_void,
  buf: &mut [f32],
) -> Result<Option<usize>, AroundError> {
  let state = unsafe { &mut *(data as *mut PcmState) };
  state.read_impl(buf)
}

unsafe fn pcm_seek(data: *mut std::ffi::c_void, frame: u64) -> Result<(), AroundError> {
  let state = unsafe { &mut *(data as *mut PcmState) };
  state.seek_impl(frame)
}

unsafe fn pcm_drop(data: *mut std::ffi::c_void) {
  unsafe { drop(Box::from_raw(data as *mut PcmState)) };
}

// ---------------------------------------------------------------------------
// Static vtable
// ---------------------------------------------------------------------------

static PCM_VTABLE: AudioStreamVTable = AudioStreamVTable {
  read: pcm_read,
  seek: pcm_seek,
  drop: pcm_drop,
};

// ---------------------------------------------------------------------------
// open_fn
// ---------------------------------------------------------------------------

fn pcm_open(reader: Box<dyn ReadSeek + Send>) -> Result<AudioStream, AroundError> {
  let (state, sample_rate) = PcmState::open(reader)?;
  let channels = state.channels;
  let total_frames = (state.samples.len() / state.channels as usize) as u64;
  let data = Box::into_raw(Box::new(state)) as *mut std::ffi::c_void;
  unsafe {
    Ok(AudioStream::new(
      data,
      &PCM_VTABLE,
      sample_rate,
      channels,
      total_frames,
    ))
  }
}

// ---------------------------------------------------------------------------
// Public codec info
// ---------------------------------------------------------------------------

/// Static codec descriptor. The engine calls `registry.push(PCM_INFO)`.
pub static PCM_INFO: CodecInfo = CodecInfo::new("pcm", PCM_FORMATS, pcm_open);

// ---------------------------------------------------------------------------
// FFI entry point for dynamic loading
// ---------------------------------------------------------------------------

/// FFI entry point for dynamic loading.
/// Returns a C-ABI-compatible codec descriptor.
#[repr(C)]
pub struct CodecInfoFFI {
  pub name: *const u8,
  pub name_len: usize,
  pub extensions: *const u8,
  pub extensions_len: usize,
  pub open_fn: unsafe extern "C" fn(reader: *mut std::ffi::c_void) -> *mut std::ffi::c_void,
}

#[no_mangle]
pub extern "C" fn get_codec_info() -> CodecInfoFFI {
  CodecInfoFFI {
    name: b"pcm\0".as_ptr(),
    name_len: 3,
    extensions: b"pcm\0".as_ptr(),
    extensions_len: 3,
    open_fn: pcm_open_ffi,
  }
}

/// FFI-compatible open wrapper. `reader` is a `*mut Box<dyn ReadSeek + Send + Sync>`.
/// Stub — dynamic codec loading delegates to `pcm_open` via the vtable path.
unsafe extern "C" fn pcm_open_ffi(_reader: *mut std::ffi::c_void) -> *mut std::ffi::c_void {
  std::ptr::null_mut()
}
