//! around-codec-wav: Built-in WAV decoder.

use around_core::{
  AroundError, Decoder, FormatSignature, Metadata, SampleSpec, Source, SourceRequirements,
};
use std::io::Read;
use std::sync::LazyLock;

static WAV_FORMATS: LazyLock<Vec<FormatSignature>> = LazyLock::new(|| {
  vec![FormatSignature::from_extension("wav", "WAV audio")
    .with_mime("audio/wav")
    .with_magic(b"RIFF")]
});

pub struct WavDecoder {
  metadata: Metadata,
  output_format: SampleSpec,
  samples: Vec<f32>,
  position: usize,
}

impl WavDecoder {
  /// Decode all samples from the source at open time (simple approach for MVP).
  fn decode_all(source: &dyn Source) -> Result<(Vec<f32>, SampleSpec, Metadata), AroundError> {
    let mut reader = source.open()?;
    let mut raw = Vec::new();
    reader
      .read_to_end(&mut raw)
      .map_err(|e| AroundError::DecodeError {
        message: format!("failed to read source: {}", e),
      })?;

    if raw.len() < 44 {
      return Err(AroundError::DecodeError {
        message: "file too small to be valid WAV".into(),
      });
    }

    if &raw[0..4] != b"RIFF" || &raw[8..12] != b"WAVE" {
      return Err(AroundError::DecodeError {
        message: "not a valid WAV file".into(),
      });
    }

    // Find "fmt " chunk. Chunk structure: [4B ID][4B size][data…]
    let fmt_pos = raw[12..]
      .windows(4)
      .position(|w| w == b"fmt ")
      .map(|p| p + 12)
      .ok_or_else(|| AroundError::DecodeError {
        message: "WAV file missing fmt chunk".into(),
      })?;

    // Skip 4B chunk ID + 4B chunk size → data at +8
    let audio_format = u16::from_le_bytes([raw[fmt_pos + 8], raw[fmt_pos + 9]]);
    if audio_format != 1 {
      return Err(AroundError::DecodeError {
        message: format!(
          "unsupported WAV format: {} (only PCM=1 supported)",
          audio_format
        ),
      });
    }
    let num_channels = u16::from_le_bytes([raw[fmt_pos + 10], raw[fmt_pos + 11]]) as u8;
    let sample_rate = u32::from_le_bytes([
      raw[fmt_pos + 12],
      raw[fmt_pos + 13],
      raw[fmt_pos + 14],
      raw[fmt_pos + 15],
    ]);
    let bits_per_sample = u16::from_le_bytes([raw[fmt_pos + 22], raw[fmt_pos + 23]]);

    // Find "data" chunk
    let data_pos = raw[fmt_pos..]
      .windows(4)
      .position(|w| w == b"data")
      .map(|p| p + fmt_pos)
      .ok_or_else(|| AroundError::DecodeError {
        message: "WAV file missing data chunk".into(),
      })?;

    let data_size = u32::from_le_bytes([
      raw[data_pos + 4],
      raw[data_pos + 5],
      raw[data_pos + 6],
      raw[data_pos + 7],
    ]) as usize;

    let data_start = data_pos + 8;
    let data_end = std::cmp::min(data_start + data_size, raw.len());
    let pcm_data = &raw[data_start..data_end];

    let samples: Vec<f32> = match bits_per_sample {
      8 => pcm_data.iter().map(|&b| (b as f32 / 128.0) - 1.0).collect(),
      16 => {
        let num_samples = pcm_data.len() / 2;
        let mut out = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
          let sample = i16::from_le_bytes([pcm_data[i * 2], pcm_data[i * 2 + 1]]);
          out.push(sample as f32 / 32768.0);
        }
        out
      }
      24 => {
        let num_samples = pcm_data.len() / 3;
        let mut out = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
          let b0 = pcm_data[i * 3] as i32;
          let b1 = pcm_data[i * 3 + 1] as i32;
          let b2 = pcm_data[i * 3 + 2] as i32;
          let val = b0 | (b1 << 8) | (b2 << 16);
          let val = if val & 0x800000 != 0 {
            val | !0xffffff
          } else {
            val
          };
          out.push(val as f32 / 8388608.0);
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

    let spec = SampleSpec::new(sample_rate, num_channels, bits_per_sample as u8).map_err(|e| {
      AroundError::DecodeError {
        message: e.to_string(),
      }
    })?;

    let metadata = Metadata::new();
    Ok((samples, spec, metadata))
  }
}

impl Decoder for WavDecoder {
  fn supported_formats() -> &'static [FormatSignature] {
    &WAV_FORMATS
  }

  fn source_requirements() -> SourceRequirements {
    SourceRequirements::SEEKABLE | SourceRequirements::KNOWN_LENGTH
  }

  fn can_decode(source: &dyn Source) -> bool {
    let id = source.identifier().to_lowercase();
    if id.ends_with(".wav") || id.ends_with(".wave") {
      return true;
    }
    if let Ok(mut reader) = source.open() {
      let mut magic = [0u8; 4];
      if reader.read_exact(&mut magic).is_ok() && &magic == b"RIFF" {
        return true;
      }
    }
    false
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
