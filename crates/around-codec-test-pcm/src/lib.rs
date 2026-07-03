//! around-codec-test-pcm: Minimal PCM codec for extension contract testing.
//!
//! Implements the ABI-stable [`Codec`] trait for raw PCM data with a simple
//! header format:
//! - 4 bytes: sample rate (u32 LE)
//! - 1 byte: channels (u8)
//! - 1 byte: bits per sample (u8)
//! - Remainder: raw PCM samples (interleaved if multi-channel)

use std::ffi::c_void;

// ---------------------------------------------------------------------------
// Extension metadata
// ---------------------------------------------------------------------------

#[no_mangle]
pub static AROUND_META: around_extensions::ExtensionMeta = around_extensions::ExtensionMeta {
  name: c"around-codec-test-pcm".as_ptr().cast(),
  semver: c"0.1.0".as_ptr().cast(),
  api_version: 1,
  _pad: 0,
  depends_on: std::ptr::null(),
  depends_on_slots: std::ptr::null(),
  author: c"around project".as_ptr().cast(),
  description: c"Test PCM codec for extension contract testing"
    .as_ptr()
    .cast(),
};

// ---------------------------------------------------------------------------
// Codec implementation
// ---------------------------------------------------------------------------

/// Stateless PCM codec. Per-stream state is allocated in `open()`.
pub struct PcmCodec;

struct PcmStream {
  reader_ctx: *mut c_void,
  reader_read: around_core::codec::ReadFn,
  channels: u8,
  bit_depth: u8,
  position: u64,
  raw_buf: Vec<u8>,
}
impl PcmStream {}

#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl around_core::codec::Codec for PcmCodec {
  extern "C" fn probe(
    &self,
    header: *const u8,
    header_len: usize,
    filename: *const u8,
    filename_len: usize,
  ) -> u8 {
    // PCM doesn't have a magic header. Check by extension.
    if filename_len >= 4 {
      let name = unsafe { std::slice::from_raw_parts(filename, filename_len) };
      let name_str = std::str::from_utf8(name).unwrap_or("");
      if name_str.to_lowercase().ends_with(".pcm") {
        return 70;
      }
    }
    // Check if header looks like valid PCM params.
    if header_len >= 6 {
      let hdr = unsafe { std::slice::from_raw_parts(header, 6) };
      let sample_rate = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
      let channels = hdr[4];
      let bit_depth = hdr[5];
      if sample_rate > 0
        && sample_rate <= 384000
        && (1..=8).contains(&channels)
        && [8, 16, 24, 32].contains(&bit_depth)
      {
        return 50;
      }
    }
    0
  }

  extern "C" fn name(&self) -> *const u8 {
    c"pcm".as_ptr().cast()
  }

  extern "C" fn open(
    &self,
    reader_ctx: *mut c_void,
    reader_read: around_core::codec::ReadFn,
    _reader_seek: around_core::codec::SeekFn,
    out_stream: *mut *mut c_void,
    out_info: *mut around_core::codec::StreamInfo,
  ) -> i32 {
    // Read 6-byte header.
    let mut hdr = [0u8; 6];
    let ctx = reader_ctx;
    let read_fn = reader_read;

    let ret = unsafe { (read_fn)(ctx, hdr.as_mut_ptr(), 6) };
    if ret < 6 {
      return -1;
    }

    let sample_rate = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let channels = hdr[4];
    let bit_depth = hdr[5];

    if sample_rate == 0 || channels == 0 || ![8, 16, 24, 32].contains(&bit_depth) {
      return -2;
    }

    let stream = Box::new(PcmStream {
      reader_ctx,
      reader_read,
      channels,
      bit_depth,
      position: 0,
      raw_buf: Vec::with_capacity(4096 * channels as usize * (bit_depth as usize / 8)),
    });

    unsafe {
      *out_stream = Box::into_raw(stream) as *mut c_void;
      (*out_info) = around_core::codec::StreamInfo {
        sample_rate,
        channels,
        total_frames: 0,
      };
    }
    0
  }

  extern "C" fn read(&self, stream: *mut c_void, buf: *mut f32, buf_len: usize) -> i32 {
    if stream.is_null() || buf.is_null() {
      return -1;
    }
    let s = unsafe { &mut *(stream as *mut PcmStream) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, buf_len) };
    let bytes_per_sample = (s.bit_depth as usize) / 8;
    let frame_size = (s.channels as usize) * bytes_per_sample;
    let max_frames = out.len() / s.channels as usize;

    let mut samples_written: i32 = 0;

    for _frame in 0..max_frames {
      s.raw_buf.resize(frame_size, 0);
      let buf_ptr = s.raw_buf.as_mut_ptr();
      let ctx = s.reader_ctx;
      let read_fn = s.reader_read;
      let read_ret = unsafe { (read_fn)(ctx, buf_ptr, frame_size) };

      if read_ret <= 0 || (read_ret as usize) < frame_size {
        break;
      }

      for ch in 0..s.channels as usize {
        let offset = ch * bytes_per_sample;
        let sample_f32 = match s.bit_depth {
          8 => s.raw_buf[offset] as i8 as f32 / 128.0,
          16 => i16::from_le_bytes([s.raw_buf[offset], s.raw_buf[offset + 1]]) as f32 / 32768.0,
          24 => {
            i32::from_le_bytes([
              s.raw_buf[offset],
              s.raw_buf[offset + 1],
              s.raw_buf[offset + 2],
              if s.raw_buf[offset + 2] & 0x80 != 0 {
                0xff
              } else {
                0
              },
            ]) as f32
              / 8388608.0
          }
          32 => {
            i32::from_le_bytes([
              s.raw_buf[offset],
              s.raw_buf[offset + 1],
              s.raw_buf[offset + 2],
              s.raw_buf[offset + 3],
            ]) as f32
              / 2147483648.0
          }
          _ => 0.0,
        };
        let idx = samples_written as usize + ch;
        if idx < out.len() {
          out[idx] = sample_f32;
        }
      }
      samples_written += s.channels as i32;
      s.position += 1;
    }

    samples_written
  }

  extern "C" fn seek(&self, _stream: *mut c_void, _frame: u64) -> i64 {
    -1 // Raw PCM streams are not seekable.
  }

  extern "C" fn drop(&self, stream: *mut c_void) {
    if !stream.is_null() {
      unsafe {
        drop(Box::from_raw(stream as *mut PcmStream));
      }
    }
  }
}

// ---------------------------------------------------------------------------
// FFI exports for dynamic loading
// ---------------------------------------------------------------------------

/// Creates a [`DynCodecRef`] for the PCM codec.
///
/// # Safety
///
/// Callers must ensure `index` is 0 (the only valid value). Any other value
/// returns a null pointer. The returned pointer is a heap-allocated fat
/// pointer owned by the framework; callers must not free it.
#[no_mangle]
pub unsafe extern "C" fn around_core_codec_codec_create(index: usize) -> *mut c_void {
  if index == 0 {
    let codec: around_core::codec::DynCodecRef =
      around_core::codec::DynCodecRef::from(stabby::boxed::Box::new(PcmCodec));
    std::boxed::Box::into_raw(std::boxed::Box::new(codec)) as *mut c_void
  } else {
    std::ptr::null_mut()
  }
}
