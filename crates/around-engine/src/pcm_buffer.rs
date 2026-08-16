//! PcmBuffer — pre-allocated region layout for zero-allocation decode pipeline.
//!
//! The buffer is divided into two contiguous regions:
//! - Decode region: where the codec writes raw PCM
//! - Resample region: where resampling/filtering operates
//!
//! After construction, no heap allocations occur during use.

pub struct PcmBuffer {
  buf: Vec<f32>,
  /// End index of the decode region (exclusive).
  decoder_max: usize,
  /// Start index of the resample region.
  resample_offset: usize,
}

impl PcmBuffer {
  /// Create a new buffer with the given maximum sample counts.
  /// `decoder_max_samples` bounds the decode output per iteration.
  /// `resample_max_samples` bounds the resample/filter output.
  pub fn new(decoder_max_samples: usize, resample_max_samples: usize) -> Self {
    let Some(total) = decoder_max_samples.checked_add(resample_max_samples) else {
      panic!("PcmBuffer region sizes overflow usize");
    };
    Self {
      buf: vec![0.0f32; total],
      decoder_max: decoder_max_samples,
      resample_offset: decoder_max_samples,
    }
  }

  /// Mutable slice for the decode region: `[0..decoder_max]`.
  pub fn decode_region(&mut self) -> &mut [f32] {
    &mut self.buf[..self.decoder_max]
  }

  /// Mutable slice for the resample region: `[decoder_max..]`.
  pub fn resample_region(&mut self) -> &mut [f32] {
    &mut self.buf[self.resample_offset..]
  }

  /// The full buffer as a mutable slice.
  pub fn as_mut_slice(&mut self) -> &mut [f32] {
    &mut self.buf
  }
}

impl Default for PcmBuffer {
  fn default() -> Self {
    Self::new(0, 0)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn regions_dont_overlap() {
    let mut buf = PcmBuffer::new(1024, 512);
    assert_eq!(buf.buf.len(), 1024 + 512);
    // Verify decode region size.
    assert_eq!(buf.decode_region().len(), 1024);
    // Verify resample region size.
    assert_eq!(buf.resample_region().len(), 512);
  }

  #[test]
  fn sizes_match_input() {
    let buf = PcmBuffer::new(4096, 2048);
    assert_eq!(buf.buf.len(), 4096 + 2048);
  }
}
