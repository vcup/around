//! around-codec-test-pcm: Minimal PCM decoder for extension contract testing.
//!
//! Handles raw PCM data with a simple header format:
//! - 4 bytes: sample rate (u32 LE)
//! - 1 byte: channels (u8)
//! - 1 byte: bits per sample (u8)
//! - Remainder: raw PCM samples (interleaved if multi-channel)

use around_core::{
  AroundError, Decoder, DecoderFactory, FormatSignature, Metadata, SampleSpec, Source,
  SourceRequirements,
};
use std::sync::LazyLock;

static PCM_FORMATS: LazyLock<Vec<FormatSignature>> = LazyLock::new(|| {
  vec![FormatSignature::from_extension(
    "pcm",
    "Raw PCM with header",
  )]
});

pub struct PcmDecoder {
  metadata: Metadata,
  output_format: SampleSpec,
  samples: Vec<f32>,
  position: usize,
}

impl PcmDecoder {
  fn decode_all(source: &dyn Source) -> Result<(Vec<f32>, SampleSpec, Metadata), AroundError> {
    let mut reader = source.open()?;
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

    let spec = SampleSpec::new(sample_rate, channels, bits_per_sample).map_err(|e| {
      AroundError::DecodeError {
        message: e.to_string(),
      }
    })?;

    Ok((samples, spec, Metadata::new()))
  }
}

impl Decoder for PcmDecoder {
  fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError> {
    if self.position >= self.samples.len() {
      return Ok(None);
    }
    let remaining = self.samples.len() - self.position;
    let to_copy = std::cmp::min(remaining, buf.len());
    buf[..to_copy].copy_from_slice(&self.samples[self.position..self.position + to_copy]);
    self.position += to_copy;
    Ok(Some(to_copy))
  }

  fn seek(&mut self, offset: u64) -> Result<(), AroundError> {
    let offset = offset as usize;
    if offset > self.samples.len() {
      return Err(AroundError::DecodeError {
        message: format!(
          "seek offset {} exceeds {} samples",
          offset,
          self.samples.len()
        ),
      });
    }
    self.position = offset;
    Ok(())
  }

  fn metadata(&self) -> &Metadata {
    &self.metadata
  }

  fn output_format(&self) -> SampleSpec {
    self.output_format
  }
}

impl DecoderFactory for PcmDecoder {
  fn supported_formats() -> &'static [FormatSignature] {
    &PCM_FORMATS
  }

  fn source_requirements() -> SourceRequirements {
    SourceRequirements::KNOWN_LENGTH
  }

  fn can_decode(source: &dyn Source) -> bool {
    source.identifier().to_lowercase().ends_with(".pcm")
  }

  fn open(source: Box<dyn Source>) -> Result<Self, AroundError> {
    let (samples, output_format, metadata) = Self::decode_all(source.as_ref())?;
    Ok(Self {
      metadata,
      output_format,
      samples,
      position: 0,
    })
  }
}

/// FFI entry point for dynamic loading.
/// The extension manager calls this symbol after loading the shared library.
#[no_mangle]
pub extern "C" fn create_decoder() -> Box<around_core::ErasedDecoder> {
  Box::new(around_core::ErasedDecoder {
    supported_formats: PcmDecoder::supported_formats(),
    source_requirements: PcmDecoder::source_requirements(),
    inner: Box::new(PcmDecoder {
      metadata: Metadata::new(),
      output_format: SampleSpec {
        sample_rate: 0,
        channels: 0,
        bit_depth: 0,
      },
      samples: Vec::new(),
      position: 0,
    }),
  })
}
