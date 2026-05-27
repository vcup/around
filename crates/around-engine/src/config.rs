//! Configuration loading and format detection.

use around_core::{AroundError, AudioFormat, FormatSignature, SampleSpec, Source};
use std::path::Path;

/// Detected format info returned by format detection.
#[derive(Debug, Clone)]
pub struct DetectedFormat {
  pub format: AudioFormat,
  pub signature: FormatSignature,
}

/// Detect the audio format of a source.
/// Strategy: extension-first, magic bytes fallback, magic-bytes-wins-with-warning.
pub fn detect_format(source: &dyn Source) -> Result<DetectedFormat, AroundError> {
  let id = source.identifier();
  let path = Path::new(&id);

  // Try extension-based detection first
  let ext_hint = path
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| e.to_lowercase());

  // Try magic bytes if we can open the source
  let magic_hint = source.open().ok().and_then(|mut reader| {
    let mut magic = [0u8; 12];
    let n = std::io::Read::read(&mut reader, &mut magic).unwrap_or(0);
    if n >= 4 {
      Some(magic)
    } else {
      None
    }
  });

  match (&ext_hint, &magic_hint) {
    (Some(ext), Some(magic)) if &magic[0..4] == b"RIFF" && &magic[8..12] == b"WAVE" => {
      // Magic bytes confirm WAV
      if ext != "wav" && ext != "wave" {
        tracing::warn!(
          "extension '.{}' does not match magic bytes (WAV), using magic bytes",
          ext
        );
      }
      Ok(DetectedFormat {
        format: AudioFormat {
          container: "WAV".into(),
          codec: "PCM".into(),
          mime_type: "audio/wav".into(),
          sample_spec: SampleSpec {
            sample_rate: 0,
            channels: 0,
            bit_depth: 0,
          },
          bitrate: None,
        },
        signature: FormatSignature::from_extension("wav", "WAV audio")
          .with_mime("audio/wav")
          .with_magic(b"RIFF"),
      })
    }
    (Some(ext), _) if ext == "wav" || ext == "wave" => Ok(DetectedFormat {
      format: AudioFormat {
        container: "WAV".into(),
        codec: "PCM".into(),
        mime_type: "audio/wav".into(),
        sample_spec: SampleSpec {
          sample_rate: 0,
          channels: 0,
          bit_depth: 0,
        },
        bitrate: None,
      },
      signature: FormatSignature::from_extension("wav", "WAV audio")
        .with_mime("audio/wav")
        .with_magic(b"RIFF"),
    }),
    _ => Err(AroundError::UnsupportedFormat {
      format: ext_hint,
      reason: "no matching decoder found".into(),
    }),
  }
}

/// Select a decoder for the given source.
/// Currently only WAV is built-in. Returns the decoder name.
pub fn select_decoder(source: &dyn Source) -> Result<&'static str, AroundError> {
  let id = source.identifier().to_lowercase();
  if id.ends_with(".wav") || id.ends_with(".wave") {
    return Ok("wav-builtin");
  }
  // Try magic bytes
  if let Ok(mut reader) = source.open() {
    let mut magic = [0u8; 12];
    if std::io::Read::read(&mut reader, &mut magic).is_ok()
      && &magic[0..4] == b"RIFF"
      && &magic[8..12] == b"WAVE"
    {
      return Ok("wav-builtin");
    }
  }
  Err(AroundError::UnsupportedFormat {
    format: None,
    reason: "no decoder found for this format".into(),
  })
}

/// KDL-backed engine configuration.
/// Full KDL file loading deferred to a later phase.
#[derive(Debug, Clone)]
pub struct EngineConfig {
  pub log_level: String,
  pub output_device: Option<String>,
  pub decoder_paths: Vec<String>,
}

impl Default for EngineConfig {
  fn default() -> Self {
    Self {
      log_level: "info".into(),
      output_device: None,
      decoder_paths: vec!["~/.local/share/around/decoders".into()],
    }
  }
}

impl EngineConfig {
  /// Load config from compiled defaults. KDL file and CLI overrides are deferred.
  pub fn load() -> Self {
    Self::default()
  }
}
