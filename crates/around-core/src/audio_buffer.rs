//! Borrowed audio buffers with explicit PCM format metadata.
//!
//! `AudioBufferRef` and `AudioBufferMut` are the in-process representation used
//! by FilterChain and OutputBinding. Data is always a byte slice so packed
//! 24/48-bit encodings remain exact; typed helpers validate the declared
//! encoding before exposing samples.

use crate::{Interleave, PcmEncoding, SampleSpec};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioBufferError {
  Spec(String),
  EncodingMismatch {
    expected: PcmEncoding,
    actual: PcmEncoding,
  },
  InterleaveMismatch,
  FrameAlignment {
    bytes: usize,
    frame_bytes: usize,
  },
  LengthMismatch {
    expected: usize,
    actual: usize,
  },
  NullData,
}

impl fmt::Display for AudioBufferError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Spec(message) => write!(f, "invalid audio buffer spec: {message}"),
      Self::EncodingMismatch { expected, actual } => {
        write!(
          f,
          "audio buffer encoding mismatch: expected {expected:?}, got {actual:?}"
        )
      }
      Self::InterleaveMismatch => f.write_str("audio buffer interleave mismatch"),
      Self::FrameAlignment { bytes, frame_bytes } => {
        write!(
          f,
          "audio buffer has {bytes} bytes, not aligned to {frame_bytes}-byte frames"
        )
      }
      Self::LengthMismatch { expected, actual } => {
        write!(
          f,
          "audio buffer length mismatch: expected {expected}, got {actual}"
        )
      }
      Self::NullData => f.write_str("audio buffer data pointer is null"),
    }
  }
}

impl std::error::Error for AudioBufferError {}

#[derive(Debug, Clone, Copy)]
pub struct AudioBufferRef<'a> {
  spec: SampleSpec,
  frames: usize,
  data: &'a [u8],
}

impl<'a> AudioBufferRef<'a> {
  pub fn new(spec: SampleSpec, frames: usize, data: &'a [u8]) -> Result<Self, AudioBufferError> {
    let expected = frames
      .checked_mul(spec.bytes_per_frame())
      .ok_or_else(|| AudioBufferError::Spec("frame count overflows byte length".into()))?;
    if expected != data.len() {
      return Err(AudioBufferError::LengthMismatch {
        expected,
        actual: data.len(),
      });
    }
    Ok(Self { spec, frames, data })
  }

  /// Construct a view over interleaved f32 samples without copying.
  pub fn from_f32_interleaved(
    sample_rate: u32,
    channels: u8,
    samples: &'a [f32],
  ) -> Result<Self, AudioBufferError> {
    let spec = SampleSpec::interleaved(sample_rate, channels, PcmEncoding::F32)
      .map_err(|message| AudioBufferError::Spec(message.into()))?;
    if samples.len() % usize::from(channels) != 0 {
      return Err(AudioBufferError::FrameAlignment {
        bytes: samples.len() * 4,
        frame_bytes: spec.bytes_per_frame(),
      });
    }
    let data = unsafe {
      std::slice::from_raw_parts(
        samples.as_ptr().cast::<u8>(),
        std::mem::size_of_val(samples),
      )
    };
    Self::new(spec, samples.len() / usize::from(channels), data)
  }

  pub fn spec(&self) -> SampleSpec {
    self.spec
  }

  pub fn frames(&self) -> usize {
    self.frames
  }

  pub fn data(&self) -> &'a [u8] {
    self.data
  }

  pub fn as_f32_interleaved(&self) -> Result<&'a [f32], AudioBufferError> {
    if self.spec.encoding != PcmEncoding::F32 {
      return Err(AudioBufferError::EncodingMismatch {
        expected: PcmEncoding::F32,
        actual: self.spec.encoding,
      });
    }
    if self.spec.interleave != Interleave::Interleaved {
      return Err(AudioBufferError::InterleaveMismatch);
    }
    if self.data.as_ptr().align_offset(std::mem::align_of::<f32>()) != 0 {
      return Err(AudioBufferError::Spec("f32 data is not aligned".into()));
    }
    Ok(unsafe { std::slice::from_raw_parts(self.data.as_ptr().cast::<f32>(), self.data.len() / 4) })
  }

  pub fn validate(&self) -> Result<(), AudioBufferError> {
    Self::new(self.spec, self.frames, self.data).map(|_| ())
  }
}

#[derive(Debug)]
pub struct AudioBufferMut<'a> {
  spec: SampleSpec,
  frames: usize,
  data: &'a mut [u8],
}

impl<'a> AudioBufferMut<'a> {
  pub fn new(
    spec: SampleSpec,
    frames: usize,
    data: &'a mut [u8],
  ) -> Result<Self, AudioBufferError> {
    let expected = frames
      .checked_mul(spec.bytes_per_frame())
      .ok_or_else(|| AudioBufferError::Spec("frame count overflows byte length".into()))?;
    if expected != data.len() {
      return Err(AudioBufferError::LengthMismatch {
        expected,
        actual: data.len(),
      });
    }
    Ok(Self { spec, frames, data })
  }

  /// Construct a mutable view over interleaved f32 samples without copying.
  pub fn from_f32_interleaved(
    sample_rate: u32,
    channels: u8,
    samples: &'a mut [f32],
  ) -> Result<Self, AudioBufferError> {
    let spec = SampleSpec::interleaved(sample_rate, channels, PcmEncoding::F32)
      .map_err(|message| AudioBufferError::Spec(message.into()))?;
    if samples.len() % usize::from(channels) != 0 {
      return Err(AudioBufferError::FrameAlignment {
        bytes: samples.len() * 4,
        frame_bytes: spec.bytes_per_frame(),
      });
    }
    let data = unsafe {
      std::slice::from_raw_parts_mut(
        samples.as_mut_ptr().cast::<u8>(),
        std::mem::size_of_val(samples),
      )
    };
    Self::new(spec, samples.len() / usize::from(channels), data)
  }

  pub fn spec(&self) -> SampleSpec {
    self.spec
  }

  pub fn frames(&self) -> usize {
    self.frames
  }

  pub fn data(&self) -> &[u8] {
    self.data
  }

  pub fn data_mut(&mut self) -> &mut [u8] {
    self.data
  }

  pub fn as_f32_interleaved_mut(&mut self) -> Result<&mut [f32], AudioBufferError> {
    if self.spec.encoding != PcmEncoding::F32 {
      return Err(AudioBufferError::EncodingMismatch {
        expected: PcmEncoding::F32,
        actual: self.spec.encoding,
      });
    }
    if self.spec.interleave != Interleave::Interleaved {
      return Err(AudioBufferError::InterleaveMismatch);
    }
    if self.data.as_ptr().align_offset(std::mem::align_of::<f32>()) != 0 {
      return Err(AudioBufferError::Spec("f32 data is not aligned".into()));
    }
    Ok(unsafe {
      std::slice::from_raw_parts_mut(self.data.as_mut_ptr().cast::<f32>(), self.data.len() / 4)
    })
  }

  pub fn validate(&self) -> Result<(), AudioBufferError> {
    let expected = self.frames * self.spec.bytes_per_frame();
    if expected == self.data.len() {
      Ok(())
    } else {
      Err(AudioBufferError::LengthMismatch {
        expected,
        actual: self.data.len(),
      })
    }
  }
}

/// Convert a possibly non-native byte order to native-order bytes in place.
pub fn normalize_byte_order(spec: SampleSpec, data: &mut [u8]) -> Result<(), AudioBufferError> {
  if spec.byte_order.is_native() || spec.bytes_per_sample() == 1 {
    return Ok(());
  }
  let width = spec.bytes_per_sample();
  if width != 0 && data.len() % width != 0 {
    return Err(AudioBufferError::FrameAlignment {
      bytes: data.len(),
      frame_bytes: width,
    });
  }
  for sample in data.chunks_exact_mut(width) {
    sample.reverse();
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  #![expect(
    clippy::unwrap_used,
    clippy::drop_non_drop,
    reason = "validated audio buffer fixtures make these test unwraps deterministic"
  )]
  use super::*;
  use crate::ByteOrder;

  #[test]
  fn f32_view_round_trips_without_copy() {
    let mut samples = [0.25_f32, -0.5, 1.0, 0.0];
    let buffer = AudioBufferMut::from_f32_interleaved(48000, 2, &mut samples).unwrap();
    assert_eq!(buffer.spec().encoding, PcmEncoding::F32);
    assert_eq!(buffer.frames(), 2);
    drop(buffer);
    let view = AudioBufferRef::from_f32_interleaved(48000, 2, &samples).unwrap();
    assert_eq!(view.as_f32_interleaved().unwrap(), &samples);
  }

  #[test]
  fn packed_lengths_are_exact() {
    let spec = SampleSpec::interleaved(48000, 2, PcmEncoding::I24).unwrap();
    assert!(AudioBufferRef::new(spec, 3, &[0; 18]).is_ok());
    assert!(AudioBufferRef::new(spec, 3, &[0; 24]).is_err());
  }

  #[test]
  fn byte_order_normalization_reverses_each_sample() {
    let spec = SampleSpec::new(
      48000,
      1,
      PcmEncoding::I16,
      Interleave::Interleaved,
      if cfg!(target_endian = "little") {
        ByteOrder::Big
      } else {
        ByteOrder::Little
      },
    )
    .unwrap();
    let mut data = [0x12, 0x34, 0x56, 0x78];
    normalize_byte_order(spec, &mut data).unwrap();
    assert_eq!(data, [0x34, 0x12, 0x78, 0x56]);
  }
}
