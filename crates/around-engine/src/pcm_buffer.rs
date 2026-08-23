//! PcmBuffer — pre-allocated workspace for decode and terminal conversion.

pub struct PcmBuffer {
  decode: Vec<f32>,
  filter: Vec<f32>,
  scratch: Vec<f32>,
  output: Vec<u8>,
}

impl PcmBuffer {
  /// Create a workspace. The f32 processing regions are sized from the larger
  /// of the decoded sample count and the byte output capacity.
  pub fn new(decoder_max_samples: usize, output_max_bytes: usize) -> Self {
    let processing = decoder_max_samples.max(output_max_bytes / std::mem::size_of::<f32>());
    Self::with_processing_capacity(decoder_max_samples, processing, output_max_bytes)
  }

  pub fn with_processing_capacity(
    decoder_max_samples: usize,
    processing_max_samples: usize,
    output_max_bytes: usize,
  ) -> Self {
    Self {
      decode: vec![0.0; decoder_max_samples],
      filter: vec![0.0; processing_max_samples],
      scratch: vec![0.0; processing_max_samples],
      output: vec![0; output_max_bytes],
    }
  }

  pub fn decode_region(&mut self) -> &mut [f32] {
    &mut self.decode
  }

  pub fn output_region(&mut self) -> &mut [u8] {
    &mut self.output
  }

  pub fn processing_regions(&mut self) -> (&mut [f32], &mut [f32], &mut [u8]) {
    (&mut self.filter, &mut self.scratch, &mut self.output)
  }

  pub fn output_capacity(&self) -> usize {
    self.output.len()
  }

  pub fn processing_capacity(&self) -> usize {
    self.filter.len()
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
  fn regions_are_preallocated_and_separate() {
    let mut buf = PcmBuffer::new(1024, 512);
    assert_eq!(buf.decode_region().len(), 1024);
    assert_eq!(buf.processing_capacity(), 1024);
    assert_eq!(buf.output_region().len(), 512);
    let (filter, scratch, output) = buf.processing_regions();
    assert_eq!(filter.len(), 1024);
    assert_eq!(scratch.len(), 1024);
    assert_eq!(output.len(), 512);
  }

  #[test]
  fn explicit_processing_capacity_supports_rate_expansion() {
    let buf = PcmBuffer::with_processing_capacity(1024, 4096, 8192);
    assert_eq!(buf.processing_capacity(), 4096);
  }
}
