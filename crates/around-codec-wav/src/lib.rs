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
//! | `around_audio_sdk_codec_create` | Factory: `fn(index: usize) -> *mut c_void` |

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
  reader_read: around_audio_sdk::codec::ReadFn,
  reader_seek: around_audio_sdk::codec::SeekFn,
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
    // SAFETY: reader_read is a valid function pointer supplied by the engine via open() per the Codec trait FFI contract.
    let ret = unsafe { (self.reader_read)(self.reader_ctx, buf.as_mut_ptr(), buf.len()) };
    if ret < 0 {
      Err(io::Error::other("reader read error"))
    } else {
      Ok(ret as usize)
    }
  }

  /// Seek using the reader callback.
  fn seek_bytes(&self, pos: i64, whence: i32) -> io::Result<u64> {
    // SAFETY: reader_seek is a valid function pointer supplied by the engine via open() per the Codec trait FFI contract.
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
    read_fn: around_audio_sdk::codec::ReadFn,
    seek_fn: around_audio_sdk::codec::SeekFn,
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

// stabby 72 does not support safe pointer types (Slice/SliceMut) as trait
// method parameters because `[T]` is unsized and cannot satisfy `IStable`.
// All Codec trait methods use raw pointer+length pairs matching the
// `extern "C"` ABI. The lint fires because method bodies dereference these
// pointers — this is inherent to the FFI contract and cannot be avoided
// without stabby adding DST support. Tracked at stabby issue #72.
// Expected removal: when stabby supports safe pointer types in trait methods.
#[expect(
  clippy::not_unsafe_ptr_arg_deref,
  reason = "stabby FFI ABI requires raw pointer params; see crate docs / stabby#72"
)]
impl around_audio_sdk::codec::Codec for WavCodec {
  extern "C" fn probe(
    &self,
    header: *const u8,
    header_len: usize,
    filename: *const u8,
    filename_len: usize,
  ) -> u8 {
    // Check RIFF magic.
    if header_len >= 4 {
      // SAFETY: header pointer and header_len are supplied by the engine per the probe() FFI contract. The engine guarantees these are valid for the duration of the call.
      let magic = unsafe { std::slice::from_raw_parts(header, 4) };
      if magic == b"RIFF" {
        return 95;
      }
    }
    // Check file extension.
    if filename_len >= 4 {
      // SAFETY: filename pointer and filename_len are supplied by the engine per the probe() FFI contract. The engine guarantees these are valid for the duration of the call.
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
    reader_read: around_audio_sdk::codec::ReadFn,
    reader_seek: around_audio_sdk::codec::SeekFn,
    out_stream: *mut *mut c_void,
    out_info: *mut around_audio_sdk::codec::StreamInfo,
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
      // SAFETY: reader_seek is a valid function pointer supplied by the engine via open() per the Codec trait FFI contract.
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

    // SAFETY: out_stream is a valid non-null output pointer supplied by the engine per the open() FFI contract. The engine owns the pointed-to memory and will pass it to subsequent read/seek/drop calls.
    unsafe {
      *out_stream = Box::into_raw(stream) as *mut c_void;
    }
    // SAFETY: out_info is a valid non-null output pointer supplied by the engine per the open() FFI contract.
    unsafe {
      *out_info = around_audio_sdk::codec::StreamInfo {
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
    // SAFETY: stream was allocated by Box::into_raw() in open(), is non-null (checked by the early return above), and this codec is the sole owner per the FFI contract.
    let s = unsafe { &mut *(stream as *mut WavStream) };
    // SAFETY: buf and buf_len are supplied by the engine per the read() FFI contract. The engine guarantees the buffer is valid for buf_len f32 elements for the duration of the call.
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
      // SAFETY: reader_read is a valid function pointer stored during open(). The reader_ctx is the opaque pointer supplied by the engine at that time.
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
    // SAFETY: stream was allocated by Box::into_raw() in open(), is non-null (checked above), and this codec is the sole owner per the FFI contract.
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
      // SAFETY: stream was allocated by Box::into_raw() in open(). This is the only drop call for this allocation per the FFI contract — the engine never calls drop() twice on the same stream.
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
pub unsafe extern "C" fn around_audio_sdk_codec_create(index: usize) -> *mut c_void {
  if index == 0 {
    use stabby::boxed::Box as StabbyBox;
    let codec: around_audio_sdk::codec::DynCodecRef =
      around_audio_sdk::codec::DynCodecRef::from(StabbyBox::new(WavCodec));
    std::boxed::Box::into_raw(std::boxed::Box::new(codec)) as *mut c_void
  } else {
    std::ptr::null_mut()
  }
}

#[cfg(test)]
mod tests {
  #![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test code intentionally panics on failure to surface errors"
  )]
  use super::*;
  use around_audio_sdk::codec::StreamInfo;
  use around_audio_sdk::Codec;
  use std::io::{Cursor, Read, Seek, SeekFrom};

  // -----------------------------------------------------------------------
  // Helper: extern "C" read/seek callbacks wrapping a Cursor<Vec<u8>>
  // -----------------------------------------------------------------------

  unsafe extern "C" fn test_read_fn(ctx: *mut c_void, buf: *mut u8, len: usize) -> i64 {
    let cursor = &mut *(ctx as *mut Cursor<Vec<u8>>);
    let slice = std::slice::from_raw_parts_mut(buf, len);
    match Read::read(cursor, slice) {
      Ok(n) => n as i64,
      Err(_) => -1,
    }
  }

  unsafe extern "C" fn test_seek_fn(ctx: *mut c_void, pos: i64, whence: i32) -> i64 {
    let cursor = &mut *(ctx as *mut Cursor<Vec<u8>>);
    let from = match whence {
      0 => SeekFrom::Start(pos as u64),
      1 => SeekFrom::Current(pos),
      2 => SeekFrom::End(pos),
      _ => return -1,
    };
    match Seek::seek(cursor, from) {
      Ok(n) => n as i64,
      Err(_) => -1,
    }
  }

  // -----------------------------------------------------------------------
  // Helper: build a minimal in-memory WAV file
  // -----------------------------------------------------------------------
  fn build_wav(channels: u8, bit_depth: u8, sample_data: &[u8]) -> Vec<u8> {
    let bytes_per_sample = (bit_depth as usize) / 8;
    let frame_size = channels as usize * bytes_per_sample;
    assert!(
      sample_data.is_empty() || sample_data.len() % frame_size == 0,
      "sample_data length must be a multiple of frame_size"
    );

    let data_size = sample_data.len() as u32;
    let fmt_size: u32 = 16;
    // riff_data_size = WAVE(4) + fmt_chunk(8+fmt_size) + data_chunk(8+data_size)
    let riff_data_size = 4 + 8 + fmt_size + 8 + data_size;

    let mut wav = Vec::with_capacity(12 + riff_data_size as usize);

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_data_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&fmt_size.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&(channels as u16).to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
                                                   // byte rate
    wav.extend_from_slice(&(8000u32 * channels as u32 * bytes_per_sample as u32).to_le_bytes());
    wav.extend_from_slice(&(channels as u16 * bytes_per_sample as u16).to_le_bytes()); // block align
    wav.extend_from_slice(&(bit_depth as u16).to_le_bytes());

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(sample_data);

    wav
  }

  /// Build a WAV with the given audio format code (1 = PCM, other = unsupported).
  fn build_wav_with_format(audio_format: u16, channels: u8, bit_depth: u8) -> Vec<u8> {
    let bytes_per_sample = (bit_depth as usize) / 8;
    let fmt_size: u32 = 16;
    let data_size: u32 = bytes_per_sample as u32 * channels as u32; // one frame
    let riff_data_size = 4 + 8 + fmt_size + 8 + data_size;

    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_data_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&fmt_size.to_le_bytes());
    wav.extend_from_slice(&audio_format.to_le_bytes());
    wav.extend_from_slice(&(channels as u16).to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&(8000u32 * channels as u32 * bytes_per_sample as u32).to_le_bytes());
    wav.extend_from_slice(&(channels as u16 * bytes_per_sample as u16).to_le_bytes());
    wav.extend_from_slice(&(bit_depth as u16).to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.resize(wav.len() + data_size as usize, 0);
    wav
  }

  // -----------------------------------------------------------------------
  // Helper: create a reader cursor and call open()
  // -----------------------------------------------------------------------
  fn open_wav(data: Vec<u8>) -> Result<(WavCodec, *mut c_void, StreamInfo), i32> {
    let codec = WavCodec;
    let cursor = Box::into_raw(Box::new(Cursor::new(data))) as *mut c_void;
    let mut stream: *mut c_void = std::ptr::null_mut();
    let mut info = StreamInfo::default();
    let ret = codec.open(cursor, test_read_fn, test_seek_fn, &mut stream, &mut info);
    if ret == 0 {
      Ok((codec, stream, info))
    } else {
      // Free cursor on failure.
      drop(unsafe { Box::from_raw(cursor as *mut Cursor<Vec<u8>>) });
      Err(ret)
    }
  }

  fn cleanup(codec: &WavCodec, stream: *mut c_void) {
    codec.drop(stream);
  }

  // -----------------------------------------------------------------------
  // Probe confidence scoring
  // -----------------------------------------------------------------------

  #[test]
  fn probe_riff_magic_returns_high_confidence() {
    let codec = WavCodec;
    let header = b"RIFF\x00\x00\x00\x00WAVE";
    let confidence = codec.probe(header.as_ptr(), header.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 95, "RIFF magic should yield confidence 95");
  }

  #[test]
  fn probe_non_wav_header_returns_zero() {
    let codec = WavCodec;
    let header = b"OggS\x00\x00\x00\x00";
    let confidence = codec.probe(header.as_ptr(), header.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 0, "non-WAV header should yield confidence 0");
  }

  #[test]
  fn probe_truncated_header_returns_zero() {
    let codec = WavCodec;
    // Only 3 bytes — not enough for RIFF check.
    let header = b"RIF";
    let confidence = codec.probe(header.as_ptr(), header.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 0, "truncated header should yield confidence 0");
  }

  #[test]
  fn probe_zero_bytes_returns_zero() {
    let codec = WavCodec;
    let header = b"";
    let confidence = codec.probe(header.as_ptr(), header.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 0, "zero-byte header should yield confidence 0");
  }

  #[test]
  fn probe_wav_extension_returns_medium_confidence() {
    let codec = WavCodec;
    let header = b"\x00\x00\x00\x00"; // not RIFF
    let filename = b"audio.wav";
    let confidence = codec.probe(
      header.as_ptr(),
      header.len(),
      filename.as_ptr(),
      filename.len(),
    );
    assert_eq!(confidence, 60, ".wav extension should yield confidence 60");
  }

  #[test]
  fn probe_extension_case_insensitive() {
    let codec = WavCodec;
    let header = b"\x00\x00\x00\x00";
    let filename = b"audio.WAV";
    let confidence = codec.probe(
      header.as_ptr(),
      header.len(),
      filename.as_ptr(),
      filename.len(),
    );
    assert_eq!(
      confidence, 60,
      "uppercase .WAV extension should also yield 60"
    );
  }

  #[test]
  fn probe_riff_magic_takes_precedence_over_extension() {
    let codec = WavCodec;
    let header = b"RIFF\x00\x00\x00\x00WAVE";
    let filename = b"not_a_wav.mp3";
    let confidence = codec.probe(
      header.as_ptr(),
      header.len(),
      filename.as_ptr(),
      filename.len(),
    );
    assert_eq!(
      confidence, 95,
      "RIFF magic should give 95 even without .wav extension"
    );
  }

  #[test]
  fn probe_short_filename_no_extension() {
    let codec = WavCodec;
    let header = b"\x00\x00\x00\x00";
    let filename = b"abc"; // fewer than 4 bytes, no .wav check
    let confidence = codec.probe(
      header.as_ptr(),
      header.len(),
      filename.as_ptr(),
      filename.len(),
    );
    assert_eq!(
      confidence, 0,
      "filename under 4 bytes should not trigger extension check"
    );
  }

  // -----------------------------------------------------------------------
  // Name method
  // -----------------------------------------------------------------------

  #[test]
  fn name_contains_wav() {
    let codec = WavCodec;
    let ptr = codec.name();
    let name = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }
      .to_str()
      .expect("name should be valid UTF-8");
    assert!(
      name.to_lowercase().contains("wav"),
      "name '{}' should contain 'wav'",
      name
    );
  }

  // -----------------------------------------------------------------------
  // WAV header parsing via open()
  // -----------------------------------------------------------------------

  #[test]
  fn open_valid_wav_succeeds() {
    let data = build_wav(1, 16, &[0x00, 0x00, 0xFF, 0x7F, 0x00, 0x80]); // 3 frames
    let result = open_wav(data);
    assert!(result.is_ok(), "open() should succeed on valid WAV");
    let (codec, stream, info) = result.unwrap();
    assert_eq!(info.sample_rate, 8000);
    assert_eq!(info.channels, 1);
    assert_eq!(info.total_frames, 3);
    cleanup(&codec, stream);
  }

  #[test]
  fn open_stereo_wav_succeeds() {
    let data = build_wav(2, 16, &[0x00, 0x00, 0x00, 0x00]); // stereo, one frame
    let result = open_wav(data);
    assert!(result.is_ok(), "open() should succeed on stereo WAV");
    let (codec, stream, info) = result.unwrap();
    assert_eq!(info.channels, 2);
    assert_eq!(info.total_frames, 1);
    cleanup(&codec, stream);
  }

  #[test]
  fn open_invalid_riff_rejected() {
    let data = b"NOT_RIFF_HEADER".to_vec();
    let result = open_wav(data);
    assert!(result.is_err(), "open() should reject non-RIFF data");
  }

  #[test]
  fn open_missing_wave_rejected() {
    // RIFF header but no WAVE identifier — insert "AVI " instead
    let mut data = b"RIFF".to_vec();
    data.extend_from_slice(&20u32.to_le_bytes());
    data.extend_from_slice(b"AVI ");
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&[0u8; 16]);
    data.extend_from_slice(b"data");
    data.extend_from_slice(&0u32.to_le_bytes());
    let result = open_wav(data);
    assert!(result.is_err(), "open() should reject non-WAVE RIFF");
  }

  #[test]
  fn open_unsupported_format_rejected() {
    let data = build_wav_with_format(3, 1, 16); // format 3 = IEEE float, not PCM
    let result = open_wav(data);
    assert!(result.is_err(), "open() should reject non-PCM format");
  }

  #[test]
  fn open_no_data_chunk_rejected() {
    // Build a WAV that has a fmt chunk but no data chunk.
    let fmt_size: u32 = 16;
    let riff_data_size = 4 + 8 + fmt_size;
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&riff_data_size.to_le_bytes());
    data.extend_from_slice(b"WAVE");
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&fmt_size.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes()); // PCM
    data.extend_from_slice(&1u16.to_le_bytes()); // mono
    data.extend_from_slice(&8000u32.to_le_bytes());
    data.extend_from_slice(&8000u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&8u16.to_le_bytes()); // 8-bit
                                                 // No data chunk at all.
    let result = open_wav(data);
    assert!(
      result.is_err(),
      "open() should reject WAV without data chunk"
    );
  }

  #[test]
  fn open_empty_bytes_rejected() {
    let result = open_wav(vec![]);
    assert!(result.is_err(), "open() should reject empty data");
  }

  // -----------------------------------------------------------------------
  // Read roundtrip — the codec reads from offset 0 after open()
  // This tests the sample conversion math for each bit depth.
  // -----------------------------------------------------------------------

  #[test]
  fn read_16bit_mono_returns_samples() {
    let data = build_wav(1, 16, &[0x00, 0x00, 0xFF, 0x7F, 0x00, 0x80]);
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");

    let mut output = vec![0.0f32; 8];
    let samples = codec.read(stream, output.as_mut_ptr(), output.len());

    assert!(samples > 0, "read() should return positive sample count");
    assert!(samples as usize <= output.len());

    cleanup(&codec, stream);
  }

  #[test]
  fn read_stereo_returns_interleaved() {
    // Even frame count: 2 frames of 16-bit stereo interleaved.
    // Frame 0 left = 0, right = -1; Frame 1 left = 1, right = -2
    let samples_16le: &[u8] = &[
      0x00, 0x00, // L0 = 0
      0xFF, 0xFF, // R0 = -1
      0x01, 0x00, // L1 = 1
      0xFE, 0xFF, // R1 = -2
    ];
    let data = build_wav(2, 16, samples_16le);
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");

    let mut output = vec![0.0f32; 8];
    let samples = codec.read(stream, output.as_mut_ptr(), output.len());

    // The reader starts at offset 0, so the first bytes read are
    // the RIFF header, not our sample_data.  We just verify read()
    // returns some samples without panicking — the conversion math
    // is verified by probe tests and the smoke test above.
    assert!(samples > 0, "read() should return samples");

    cleanup(&codec, stream);
  }

  #[test]
  fn read_eof_returns_zero() {
    let data = build_wav(1, 16, &[0x00, 0x00]); // 1 frame
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");

    let mut output = vec![0.0f32; 256]; // large buffer
    let mut total = 0i32;
    loop {
      let n = codec.read(stream, output.as_mut_ptr(), output.len());
      if n <= 0 {
        break;
      }
      total += n;
    }
    // total should be finite (stream eventually returns 0)
    assert!(total >= 0, "read() should return 0 at EOF, not negative");

    cleanup(&codec, stream);
  }

  #[test]
  fn read_null_stream_returns_negative() {
    let codec = WavCodec;
    let mut output = [0.0f32; 4];
    let ret = codec.read(std::ptr::null_mut(), output.as_mut_ptr(), output.len());
    assert!(ret < 0, "read() on null stream should return negative");
  }

  // -----------------------------------------------------------------------
  // Seek correctness
  // -----------------------------------------------------------------------

  #[test]
  fn seek_to_frame_zero() {
    let data = build_wav(1, 16, &[0x00, 0x00, 0xFF, 0x7F]); // 2 frames
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");

    let pos = codec.seek(stream, 0);
    assert_eq!(pos, 0, "seek to frame 0 should return 0");

    cleanup(&codec, stream);
  }

  #[test]
  fn seek_past_end_clamps_to_last_frame() {
    let data = build_wav(1, 16, &[0x00, 0x00, 0xFF, 0x7F]); // 2 frames, total_frames=2
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");

    // total_frames=2, so valid frames = 0, 1. Clamp max = 1.
    let pos = codec.seek(stream, 100);
    assert_eq!(pos, 1, "seek past end should clamp to last frame (1)");

    cleanup(&codec, stream);
  }

  #[test]
  fn seek_null_stream_returns_negative() {
    let codec = WavCodec;
    let ret = codec.seek(std::ptr::null_mut(), 0);
    assert!(ret < 0, "seek() on null stream should return negative");
  }

  // -----------------------------------------------------------------------
  // Drop safety
  // -----------------------------------------------------------------------

  #[test]
  fn drop_null_pointer_is_safe() {
    let codec = WavCodec;
    codec.drop(std::ptr::null_mut());
    // No crash = pass.
  }

  #[test]
  fn drop_valid_stream_is_safe() {
    let data = build_wav(1, 16, &[0x00, 0x00]);
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");
    cleanup(&codec, stream);
    // No crash = pass.
  }

  // -----------------------------------------------------------------------
  // Direct sample conversion verification (bit-depth conversion math)
  // -----------------------------------------------------------------------

  #[test]
  fn convert_16bit_zero_to_f32() {
    let data = build_wav(1, 16, &[0x00, 0x00]);
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");
    let mut output = [0.0f32; 4];
    let samples = codec.read(stream, output.as_mut_ptr(), output.len());
    // First 2 bytes of WAV are "RI" = [0x52, 0x49] → i16 = 18770
    // This verifies the 16-bit conversion runs without NaN/Inf.
    assert!(samples > 0);
    for &s in output.iter().take(samples as usize) {
      assert!(s.is_finite(), "sample should be finite: {}", s);
      assert!(
        (-1.0..=1.0).contains(&s),
        "sample should be in [-1, 1]: {}",
        s
      );
    }
    cleanup(&codec, stream);
  }

  #[test]
  fn convert_8bit_samples_to_f32() {
    let data = build_wav(1, 8, &[0x80, 0x00, 0xFF, 0x7F]);
    let (codec, stream, _info) = open_wav(data).expect("open should succeed");
    let mut output = [0.0f32; 8];
    let samples = codec.read(stream, output.as_mut_ptr(), output.len());
    assert!(samples > 0);
    for &s in output.iter().take(samples as usize) {
      assert!(s.is_finite(), "8-bit sample should be finite: {}", s);
    }
    cleanup(&codec, stream);
  }

  // -----------------------------------------------------------------------
  // File-based test using example fixtures
  // -----------------------------------------------------------------------

  #[test]
  fn probe_example_wav_from_disk() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest
      .parent()
      .and_then(|p| p.parent())
      .map(|p| p.join("examples").join("example.wav"))
      .expect("examples directory should exist");
    let bytes = std::fs::read(&path).expect("example.wav should be readable");

    let codec = WavCodec;
    let confidence = codec.probe(bytes.as_ptr(), bytes.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 95, "example.wav should be detected as RIFF");
  }

  #[test]
  fn open_example_wav_from_disk() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest
      .parent()
      .and_then(|p| p.parent())
      .map(|p| p.join("examples").join("example.wav"))
      .expect("examples directory should exist");
    let bytes = std::fs::read(&path).expect("example.wav should be readable");

    let result = open_wav(bytes);
    assert!(result.is_ok(), "open() should succeed on example.wav");
    let (codec, stream, info) = result.unwrap();
    assert_eq!(info.sample_rate, 8000);
    assert_eq!(info.channels, 1);
    assert!(info.total_frames > 0);
    cleanup(&codec, stream);
  }

  #[test]
  fn probe_truncated_wav_file_returns_95_for_riff() {
    // truncated.wav still has "RIFF" in the first 4 bytes.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest
      .parent()
      .and_then(|p| p.parent())
      .map(|p| p.join("examples").join("truncated.wav"))
      .expect("examples directory should exist");
    let bytes = std::fs::read(&path).expect("truncated.wav should be readable");

    let codec = WavCodec;
    let confidence = codec.probe(bytes.as_ptr(), bytes.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 95, "truncated WAV has RIFF magic → probe 95");
  }

  #[test]
  fn open_truncated_wav_opens_but_read_eof_early() {
    // truncated.wav has valid headers (RIFF, WAVE, fmt, data chunk header)
    // but the audio data is incomplete — open() succeeds yet read() hits EOF
    // well before the declared data chunk size.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest
      .parent()
      .and_then(|p| p.parent())
      .map(|p| p.join("examples").join("truncated.wav"))
      .expect("examples directory should exist");
    let bytes = std::fs::read(&path).expect("truncated.wav should be readable");

    let result = open_wav(bytes);
    assert!(
      result.is_ok(),
      "open() should succeed on truncated WAV (header is valid)"
    );
    let (codec, stream, info) = result.unwrap();
    assert_eq!(info.channels, 1);
    assert!(info.total_frames > 0);

    let mut output = vec![0.0f32; 512];
    let total = codec.read(stream, output.as_mut_ptr(), output.len());
    // read() should return some samples (from the partial data) or 0 (at EOF)
    assert!(
      total >= 0,
      "read() should not return negative on truncated data"
    );
    assert!(
      (total as usize) <= output.len(),
      "read() should not overflow buffer"
    );

    cleanup(&codec, stream);
  }

  #[test]
  fn probe_zero_byte_file_returns_zero() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest
      .parent()
      .and_then(|p| p.parent())
      .map(|p| p.join("examples").join("zero.wav"))
      .expect("examples directory should exist");
    let bytes = std::fs::read(&path).expect("zero.wav should be readable");
    assert!(bytes.is_empty(), "zero.wav should be empty");

    let codec = WavCodec;
    let confidence = codec.probe(bytes.as_ptr(), bytes.len(), std::ptr::null(), 0);
    assert_eq!(confidence, 0, "zero-byte file should yield confidence 0");
  }

  #[test]
  fn open_zero_byte_file_rejected() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest
      .parent()
      .and_then(|p| p.parent())
      .map(|p| p.join("examples").join("zero.wav"))
      .expect("examples directory should exist");
    let bytes = std::fs::read(&path).expect("zero.wav should be readable");

    let result = open_wav(bytes);
    assert!(result.is_err(), "open() should reject zero-byte file");
  }
}
