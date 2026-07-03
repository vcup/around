//! around-codec-wav: WAV(PCM) codec via the Codec trait (ADR-0004 compliant).
//!
//! Implements the ABI-stable [`Codec`] trait for RIFF/WAV PCM audio files.
//! Uses reader callbacks (`ReadFn`/`SeekFn`) during `open()` instead of
//! `Box<dyn ReadSeek>`, enabling cross-FFI-boundary streaming.
//!
//! ## Extension exports
//!
//! | Symbol | Purpose |
//! |---|---|
//! | `AROUND_META` | Extension metadata (name, version, dependencies) |
//! | `around_codec_create` | Factory: `fn(index: usize) -> *mut c_void` |

use std::ffi::c_void;
use std::io;

// ---------------------------------------------------------------------------
// Extension metadata
// ---------------------------------------------------------------------------

/// Maximum frames decoded per internal read batch — bounds per-call memory.
const READ_BATCH_FRAMES: usize = 4096;

/// Exported extension metadata.
#[no_mangle]
pub static AROUND_META: around_extensions::ExtensionMeta = around_extensions::ExtensionMeta {
  name: c"around-codec-wav".as_ptr().cast(),
  semver: c"0.1.0".as_ptr().cast(),
  api_version: 1,
  _pad: 0,
  depends_on: std::ptr::null(),
  depends_on_slots: std::ptr::null(),
  author: c"around project".as_ptr().cast(),
  description: c"WAV PCM audio codec".as_ptr().cast(),
};

// ---------------------------------------------------------------------------
// Codec implementation
// ---------------------------------------------------------------------------

/// Stateless WAV codec. Per-stream state is allocated in `open()` and stored
/// as an opaque `*mut c_void`.
pub struct WavCodec;

/// Per-stream state for WAV decoding.
struct WavStream {
  reader_ctx: *mut c_void,
  reader_read: around_core::codec::ReadFn,
  reader_seek: around_core::codec::SeekFn,
  channels: u8,
  bit_depth: u8,
  data_start: u64,
  bytes_per_frame: usize,
  total_frames: u64,
  position: u64,
  raw_buf: Vec<u8>,
}

impl WavStream {
  /// Read from the reader callback.
  fn read_bytes(&self, buf: &mut [u8]) -> io::Result<usize> {
    let ret = unsafe { (self.reader_read)(self.reader_ctx, buf.as_mut_ptr(), buf.len()) };
    if ret < 0 {
      Err(io::Error::other("reader read error"))
    } else {
      Ok(ret as usize)
    }
  }

  /// Seek using the reader callback.
  fn seek_bytes(&self, pos: i64, whence: i32) -> io::Result<u64> {
    let ret = unsafe { (self.reader_seek)(self.reader_ctx, pos, whence) };
    if ret < 0 {
      Err(io::Error::other("reader seek error"))
    } else {
      Ok(ret as u64)
    }
  }

  /// Parse RIFF/WAV header from the callback reader.
  fn parse_header(
    ctx: *mut c_void,
    read_fn: around_core::codec::ReadFn,
    seek_fn: around_core::codec::SeekFn,
  ) -> Result<(u32, u8, u8, u64, usize, u64), String> {
    // Use a temporary stream to parse the header.
    let tmp = WavStream {
      reader_ctx: ctx,
      reader_read: read_fn,
      reader_seek: seek_fn,
      channels: 0,
      bit_depth: 0,
      data_start: 0,
      bytes_per_frame: 0,
      total_frames: 0,
      position: 0,
      raw_buf: Vec::new(),
    };

    let mut riff = [0u8; 4];
    tmp
      .read_bytes(&mut riff)
      .map_err(|e| format!("read RIFF: {}", e))?;
    if &riff != b"RIFF" {
      return Err("not a RIFF file".into());
    }

    let mut size_buf = [0u8; 4];
    tmp
      .read_bytes(&mut size_buf)
      .map_err(|e| format!("read size: {}", e))?;

    let mut wave = [0u8; 4];
    tmp
      .read_bytes(&mut wave)
      .map_err(|e| format!("read WAVE: {}", e))?;
    if &wave != b"WAVE" {
      return Err("not a WAVE file".into());
    }

    let mut sample_rate: u32 = 0;
    let mut channels: u8 = 0;
    let mut bit_depth: u8 = 0;
    let mut data_start: u64 = 0;
    let mut data_size: u32 = 0;
    let mut found_fmt = false;
    let mut found_data = false;

    loop {
      let mut chunk_id = [0u8; 4];
      match tmp.read_bytes(&mut chunk_id) {
        Ok(4) => {}
        Ok(_) => break,
        Err(_) => break,
      }
      let mut len_buf = [0u8; 4];
      tmp
        .read_bytes(&mut len_buf)
        .map_err(|e| format!("read chunk len: {}", e))?;
      let chunk_len = u32::from_le_bytes(len_buf);

      match &chunk_id {
        b"fmt " => {
          let mut fmt_buf = [0u8; 16];
          let to_read = chunk_len.min(16) as usize;
          tmp
            .read_bytes(&mut fmt_buf[..to_read])
            .map_err(|e| format!("read fmt: {}", e))?;

          let audio_format = u16::from_le_bytes([fmt_buf[0], fmt_buf[1]]);
          if audio_format != 1 {
            return Err(format!("unsupported audio format: {}", audio_format));
          }
          channels = u16::from_le_bytes([fmt_buf[2], fmt_buf[3]]) as u8;
          sample_rate = u32::from_le_bytes([fmt_buf[4], fmt_buf[5], fmt_buf[6], fmt_buf[7]]);
          bit_depth = u16::from_le_bytes([fmt_buf[14], fmt_buf[15]]) as u8;
          if ![8, 16, 24, 32].contains(&bit_depth) {
            return Err(format!("unsupported bit depth: {}", bit_depth));
          }
          if channels == 0 || channels > 8 {
            return Err(format!("unsupported channel count: {}", channels));
          }
          found_fmt = true;
          // Skip remaining fmt bytes
          if chunk_len > 16 {
            let skip = (chunk_len - 16) as usize;
            tmp
              .seek_bytes(skip as i64, 1)
              .map_err(|e| format!("skip fmt extra: {}", e))?;
          }
        }
        b"data" => {
          data_start = tmp.position;
          data_size = chunk_len;
          found_data = true;
          break; // data is the last chunk we care about
        }
        _ => {
          // Skip unknown chunk
          tmp
            .seek_bytes(chunk_len as i64, 1)
            .map_err(|e| format!("skip chunk: {}", e))?;
        }
      }
    }

    if !found_fmt {
      return Err("no fmt chunk found".into());
    }
    if !found_data {
      return Err("no data chunk found".into());
    }

    let bytes_per_frame = (channels as usize) * (bit_depth as usize / 8);
    let total_frames = if bytes_per_frame > 0 {
      data_size as u64 / bytes_per_frame as u64
    } else {
      0
    };

    Ok((
      sample_rate,
      channels,
      bit_depth,
      data_start,
      bytes_per_frame,
      total_frames,
    ))
  }
}

// ---------------------------------------------------------------------------
// Codec trait implementation
// ---------------------------------------------------------------------------

#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl around_core::codec::Codec for WavCodec {
  extern "C" fn probe(
    &self,
    header: *const u8,
    header_len: usize,
    filename: *const u8,
    filename_len: usize,
  ) -> u8 {
    // Check RIFF magic.
    if header_len >= 4 {
      let magic = unsafe { std::slice::from_raw_parts(header, 4) };
      if magic == b"RIFF" {
        return 95;
      }
    }
    // Check file extension.
    if filename_len >= 4 {
      let name = unsafe { std::slice::from_raw_parts(filename, filename_len) };
      let name_str = std::str::from_utf8(name).unwrap_or("");
      if name_str.to_lowercase().ends_with(".wav") {
        return 60;
      }
    }
    0
  }

  extern "C" fn name(&self) -> *const u8 {
    c"wav".as_ptr().cast()
  }

  extern "C" fn open(
    &self,
    reader_ctx: *mut c_void,
    reader_read: around_core::codec::ReadFn,
    reader_seek: around_core::codec::SeekFn,
    out_stream: *mut *mut c_void,
    out_info: *mut around_core::codec::StreamInfo,
  ) -> i32 {
    let (sample_rate, channels, bit_depth, data_start, bytes_per_frame, total_frames) =
      match WavStream::parse_header(reader_ctx, reader_read, reader_seek) {
        Ok(v) => v,
        Err(e) => {
          tracing::warn!(error = %e, "WAV open failed");
          return -1;
        }
      };

    // Seek to data_start.
    let tmp_seek = |pos: i64, whence: i32| -> Result<u64, String> {
      let ret = unsafe { (reader_seek)(reader_ctx, pos, whence) };
      if ret < 0 {
        Err("seek failed".into())
      } else {
        Ok(ret as u64)
      }
    };
    if tmp_seek(data_start as i64, 0).is_err() {
      return -2;
    }

    let initial_cap = READ_BATCH_FRAMES * bytes_per_frame;

    let stream = Box::new(WavStream {
      reader_ctx,
      reader_read,
      reader_seek,
      channels,
      bit_depth,
      data_start,
      bytes_per_frame,
      total_frames,
      position: 0,
      raw_buf: Vec::with_capacity(initial_cap),
    });

    unsafe {
      *out_stream = Box::into_raw(stream) as *mut c_void;
    }
    unsafe {
      *out_info = around_core::codec::StreamInfo {
        sample_rate,
        channels,
        total_frames,
      };
    }
    0
  }

  extern "C" fn read(&self, stream: *mut c_void, buf: *mut f32, buf_len: usize) -> i32 {
    if stream.is_null() || buf.is_null() {
      return -1;
    }
    let s = unsafe { &mut *(stream as *mut WavStream) };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, buf_len) };
    let bytes_per_sample = (s.bit_depth as usize) / 8;
    let frame_size = s.channels as usize * bytes_per_sample;
    let max_frames = out.len() / s.channels as usize;

    let mut samples_written: i32 = 0;

    for _frame in 0..max_frames {
      if s.total_frames > 0 && s.position >= s.total_frames {
        break;
      }

      // Read one frame of raw bytes — extract raw fields to avoid borrow conflicts.
      s.raw_buf.resize(frame_size, 0);
      let buf_ptr = s.raw_buf.as_mut_ptr();
      let reader_ctx = s.reader_ctx;
      let reader_read = s.reader_read;
      let read_ret = unsafe { (reader_read)(reader_ctx, buf_ptr, frame_size) };
      match read_ret {
        0 => break,
        n if (n as usize) < frame_size && n >= 0 => break,
        n if n < 0 => break,
        _ => {}
      }

      // Convert samples to f32.
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
          } // 2^23
          32 => {
            i32::from_le_bytes([
              s.raw_buf[offset],
              s.raw_buf[offset + 1],
              s.raw_buf[offset + 2],
              s.raw_buf[offset + 3],
            ]) as f32
              / 2147483648.0
          } // 2^31
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

  extern "C" fn seek(&self, stream: *mut c_void, frame: u64) -> i64 {
    if stream.is_null() {
      return -1;
    }
    let s = unsafe { &mut *(stream as *mut WavStream) };

    // Clamp to valid range.
    let target = if s.total_frames > 0 {
      frame.min(s.total_frames.saturating_sub(1))
    } else {
      frame
    };

    let byte_offset = target * s.bytes_per_frame as u64;
    match s.seek_bytes((s.data_start + byte_offset) as i64, 0) {
      Ok(_) => {
        s.position = target;
        target as i64
      }
      Err(_) => -2,
    }
  }

  extern "C" fn drop(&self, stream: *mut c_void) {
    if !stream.is_null() {
      unsafe {
        drop(Box::from_raw(stream as *mut WavStream));
      }
    }
  }
}

// ---------------------------------------------------------------------------
// FFI exports for dynamic loading
// ---------------------------------------------------------------------------

/// Codec factory for dynamic loading via the extension framework.
/// Returns a `*mut c_void` pointing to a heap-allocated `DynCodecRef`
/// (stabby fat pointer). The register extracts the vtable from the Dyn layout.
/// `index` 0 returns the singleton; `index >= 1` returns null.
///
/// # Safety
///
/// Callers must ensure `index` is within the valid range (0 for the singleton)
/// and that the returned pointer is a heap-allocated `DynCodecRef` that the
/// framework takes ownership of.
#[no_mangle]
pub unsafe extern "C" fn around_core_codec_codec_create(index: usize) -> *mut c_void {
  if index == 0 {
    use stabby::boxed::Box as StabbyBox;
    let codec: around_core::codec::DynCodecRef =
      around_core::codec::DynCodecRef::from(StabbyBox::new(WavCodec));
    std::boxed::Box::into_raw(std::boxed::Box::new(codec)) as *mut c_void
  } else {
    std::ptr::null_mut()
  }
}
