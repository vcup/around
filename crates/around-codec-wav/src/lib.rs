//! around-codec-wav: Built-in WAV codec.
//!
//! Handles WAV(PCM) files via the codec system (AudioStream + CodecInfo).
//! Retains all prior fixes: C1 (frame→sample conversion), C2 (zero-size chunks),
//! H2 (bits_per_sample validation), H4 (out-of-range seek), M1 (duration).

use around_core::{
  AroundError, AudioStream, AudioStreamVTable, CodecInfo, FormatSignature, ReadSeek,
};
use std::io::{self, Read, Seek, SeekFrom};

// ---------------------------------------------------------------------------
// Format declarations
// ---------------------------------------------------------------------------

static WAV_FORMATS: &[FormatSignature] = &[FormatSignature::from_extension("wav", "WAV audio")
  .with_mime("audio/wav")
  .with_magic(b"RIFF")];

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Maximum frames decoded per internal read batch — bounds per-call memory.
const READ_BATCH_FRAMES: usize = 4096;

struct WavState {
  reader: Box<dyn ReadSeek + Send>,
  sample_rate: u32,
  channels: u8,
  bit_depth: u8,
  data_start: u64,
  bytes_per_frame: usize,
  total_frames: u64,
  position: u64,
  raw_buf: Vec<u8>,
}

impl WavState {
  fn open(mut reader: Box<dyn ReadSeek + Send>) -> Result<Self, AroundError> {
    let (sample_rate, channels, bit_depth, data_start, bytes_per_frame, total_frames) =
      Self::parse_header(&mut *reader)?;

    let initial_cap = READ_BATCH_FRAMES * bytes_per_frame;

    Ok(Self {
      reader,
      sample_rate,
      channels,
      bit_depth,
      data_start,
      bytes_per_frame,
      total_frames,
      position: 0,
      raw_buf: Vec::with_capacity(initial_cap),
    })
  }

  fn read_impl(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError> {
    if self.position >= self.total_frames {
      return Ok(None);
    }

    let ch = self.channels as usize;
    let remaining = (self.total_frames - self.position) as usize;
    let max_buf_frames = buf.len().checked_div(ch).unwrap_or(0);
    let want_frames = remaining.min(max_buf_frames);
    let bpf = self.bytes_per_frame;
    let mut total_frames_out = 0;

    while total_frames_out < want_frames {
      let batch_frames = READ_BATCH_FRAMES.min(want_frames - total_frames_out);
      let batch_bytes = batch_frames * bpf;

      if self.raw_buf.len() < batch_bytes {
        self.raw_buf.resize(batch_bytes, 0);
      }
      let n = self
        .reader
        .read(&mut self.raw_buf[..batch_bytes])
        .map_err(|e| io_err("read error during decode", e))?;

      if n == 0 {
        break;
      }

      let actual_bytes = (n / bpf) * bpf;
      if actual_bytes == 0 {
        break;
      }
      let actual_frames = actual_bytes / bpf;

      // C1: output slice uses samples (frames × channels)
      let out_offset = total_frames_out * ch;
      let out_len = actual_frames * ch;
      self.decode_samples(
        &self.raw_buf[..actual_bytes],
        &mut buf[out_offset..out_offset + out_len],
      );

      total_frames_out += actual_frames;
      self.position += actual_frames as u64;
    }

    if total_frames_out == 0 {
      Ok(None)
    } else {
      Ok(Some(total_frames_out))
    }
  }

  fn seek_impl(&mut self, offset: u64) -> Result<(), AroundError> {
    // H4: use InvalidPosition for out-of-range seeks
    if offset > self.total_frames {
      return Err(AroundError::InvalidPosition {
        position_ms: 0,
        // M1: compute duration from frame count
        duration_ms: if self.sample_rate > 0 && self.total_frames > 0 {
          Some((self.total_frames as u128 * 1000 / self.sample_rate as u128) as u64)
        } else {
          None
        },
      });
    }

    let byte_offset = self.data_start + offset * self.bytes_per_frame as u64;
    self
      .reader
      .seek(SeekFrom::Start(byte_offset))
      .map_err(|e| io_err("seek failed", e))?;

    self.position = offset;
    Ok(())
  }

  // -----------------------------------------------------------------------
  // Header parsing (unchanged from original WavDecoder)
  // -----------------------------------------------------------------------

  fn parse_header(
    reader: &mut dyn ReadSeek,
  ) -> Result<(u32, u8, u8, u64, usize, u64), AroundError> {
    // --- RIFF header ---
    let mut riff = [0u8; 12];
    reader
      .read_exact(&mut riff)
      .map_err(|e| io_err("failed to read RIFF header", e))?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
      return Err(AroundError::DecodeError {
        message: "not a valid WAV file".into(),
      });
    }

    // --- Scan chunks ---
    let mut fmt_info: Option<(u32, u8, u16)> = None;
    let mut data_start: Option<u64> = None;
    let mut data_size: u64 = 0;

    loop {
      let mut ck = [0u8; 8];
      match reader.read_exact(&mut ck) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
        Err(e) => return Err(io_err("failed to read chunk header", e)),
      }

      let ck_id: [u8; 4] = [ck[0], ck[1], ck[2], ck[3]];
      let ck_size = u32::from_le_bytes([ck[4], ck[5], ck[6], ck[7]]) as u64;

      // C2: zero-size chunk guard — skip immediately to avoid infinite loop
      if ck_size == 0 {
        continue;
      }

      match &ck_id {
        b"fmt " => {
          if fmt_info.is_some() {
            seek_rel(reader, ck_size)?;
            continue;
          }

          let to_read = std::cmp::min(ck_size, 40) as usize;
          let mut fmt_data = vec![0u8; to_read];
          reader
            .read_exact(&mut fmt_data)
            .map_err(|e| io_err("failed to read fmt chunk", e))?;
          if ck_size > 40 {
            seek_rel(reader, ck_size - 40)?;
          }

          if to_read < 16 {
            return Err(AroundError::DecodeError {
              message: "fmt chunk too small".into(),
            });
          }

          let audio_format = u16::from_le_bytes([fmt_data[0], fmt_data[1]]);
          if audio_format != 1 {
            return Err(AroundError::DecodeError {
              message: format!(
                "unsupported WAV format: {} (only PCM=1 supported)",
                audio_format
              ),
            });
          }

          let channels = u16::from_le_bytes([fmt_data[2], fmt_data[3]]);
          let sample_rate =
            u32::from_le_bytes([fmt_data[4], fmt_data[5], fmt_data[6], fmt_data[7]]);
          let bits_per_sample = u16::from_le_bytes([fmt_data[14], fmt_data[15]]);

          // H2: validate bits_per_sample
          if !matches!(bits_per_sample, 8 | 16 | 24 | 32) {
            return Err(AroundError::DecodeError {
              message: format!(
                "unsupported bits per sample: {} (supported: 8, 16, 24, 32)",
                bits_per_sample
              ),
            });
          }

          fmt_info = Some((sample_rate, channels as u8, bits_per_sample));
        }

        b"data" => {
          let pos = reader
            .stream_position()
            .map_err(|e| io_err("failed to get stream position", e))?;
          data_start = Some(pos);
          data_size = ck_size;
          seek_rel(reader, ck_size)?;
        }

        _ => {
          let skip = (ck_size + 1) & !1;
          seek_rel(reader, skip)?;
        }
      }
    }

    let (sample_rate, channels, bits_per_sample) = fmt_info.ok_or(AroundError::DecodeError {
      message: "WAV file missing fmt chunk".into(),
    })?;

    let data_start = data_start.ok_or(AroundError::DecodeError {
      message: "WAV file missing data chunk".into(),
    })?;

    reader
      .seek(SeekFrom::Start(data_start))
      .map_err(|e| io_err("failed to seek to data chunk", e))?;

    let bytes_per_frame = (bits_per_sample / 8) as usize * channels as usize;
    let total_frames = if bytes_per_frame > 0 {
      data_size / bytes_per_frame as u64
    } else {
      0
    };

    Ok((
      sample_rate,
      channels,
      bits_per_sample as u8,
      data_start,
      bytes_per_frame,
      total_frames,
    ))
  }

  // -----------------------------------------------------------------------
  // Sample decoding (unchanged from original WavDecoder)
  // -----------------------------------------------------------------------

  fn decode_samples(&self, pcm: &[u8], out: &mut [f32]) -> usize {
    match self.bit_depth {
      8 => {
        for (i, &b) in pcm.iter().enumerate() {
          out[i] = (b as f32 / 128.0) - 1.0;
        }
        pcm.len()
      }
      16 => {
        let n = pcm.len() / 2;
        for i in 0..n {
          let sample = i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]);
          out[i] = sample as f32 / 32768.0;
        }
        n
      }
      24 => {
        let n = pcm.len() / 3;
        for i in 0..n {
          let b0 = pcm[i * 3] as i32;
          let b1 = pcm[i * 3 + 1] as i32;
          let b2 = pcm[i * 3 + 2] as i32;
          let val = b0 | (b1 << 8) | (b2 << 16);
          let val = if val & 0x800000 != 0 {
            val | !0xffffff
          } else {
            val
          };
          out[i] = val as f32 / 8388608.0;
        }
        n
      }
      32 => {
        let n = pcm.len() / 4;
        for i in 0..n {
          let sample =
            i32::from_le_bytes([pcm[i * 4], pcm[i * 4 + 1], pcm[i * 4 + 2], pcm[i * 4 + 3]]);
          out[i] = sample as f32 / 2147483648.0;
        }
        n
      }
      _ => 0,
    }
  }
}

// ---------------------------------------------------------------------------
// VTable wrappers (unsafe fn → safe method)
// ---------------------------------------------------------------------------

unsafe fn wav_read(
  data: *mut std::ffi::c_void,
  buf: &mut [f32],
) -> Result<Option<usize>, AroundError> {
  let state = unsafe { &mut *(data as *mut WavState) };
  state.read_impl(buf)
}

unsafe fn wav_seek(data: *mut std::ffi::c_void, frame: u64) -> Result<(), AroundError> {
  let state = unsafe { &mut *(data as *mut WavState) };
  state.seek_impl(frame)
}

unsafe fn wav_drop(data: *mut std::ffi::c_void) {
  unsafe { drop(Box::from_raw(data as *mut WavState)) };
}

// ---------------------------------------------------------------------------
// Static vtable
// ---------------------------------------------------------------------------

static WAV_VTABLE: AudioStreamVTable = AudioStreamVTable {
  read: wav_read,
  seek: wav_seek,
  drop: wav_drop,
};

// ---------------------------------------------------------------------------
// open_fn
// ---------------------------------------------------------------------------

fn wav_open(reader: Box<dyn ReadSeek + Send>) -> Result<AudioStream, AroundError> {
  let state = WavState::open(reader)?;
  let sample_rate = state.sample_rate;
  let channels = state.channels;
  let total_frames = state.total_frames;
  let data = Box::into_raw(Box::new(state)) as *mut std::ffi::c_void;

  unsafe {
    Ok(AudioStream::new(
      data,
      &WAV_VTABLE,
      sample_rate,
      channels,
      total_frames,
    ))
  }
}

// ---------------------------------------------------------------------------
// Public codec info (engine pushes into registry explicitly)
// ---------------------------------------------------------------------------

/// Static codec descriptor. The engine calls `registry.push(WAV_INFO)`.
pub static WAV_INFO: CodecInfo = CodecInfo::new("wav", WAV_FORMATS, wav_open);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn io_err(context: &str, e: io::Error) -> AroundError {
  AroundError::DecodeError {
    message: format!("{}: {}", context, e),
  }
}

fn seek_rel(reader: &mut (impl Seek + ?Sized), offset: u64) -> Result<(), AroundError> {
  if offset == 0 {
    return Ok(());
  }
  reader
    .seek(SeekFrom::Current(offset as i64))
    .map_err(|e| io_err("seek failed", e))?;
  Ok(())
}
