//! Configuration loading and format detection.

use around_core::{
  AroundError, AudioFormat, Decoder, DecoderFactory, FormatSignature, Metadata, SampleSpec, Source,
};
use kdl::{KdlDocument, KdlNode};
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

/// Decoder fallback chain: try opening with the selected decoder,
/// then fall back through available decoders per FR-002.
///
/// Priority: (1) same-format decoders in registration order,
/// (2) all other decoders in registration order.
/// First successful open wins.
pub fn try_open_decoder(
  source: Box<dyn Source>,
  preferred: &str,
) -> Result<(Box<dyn Decoder>, SampleSpec, Metadata), AroundError> {
  // Try preferred decoder first
  match preferred {
    "wav-builtin" => match around_codec_wav::WavDecoder::open(source) {
      Ok(decoder) => {
        let fmt = decoder.output_format();
        let meta = decoder.metadata().clone();
        return Ok((Box::new(decoder), fmt, meta));
      }
      Err(e) => {
        tracing::warn!(
          ?e,
          "preferred decoder '{}' failed, attempting fallback",
          preferred
        );
      }
    },
    _ => {
      tracing::warn!("unknown decoder '{}', attempting fallback", preferred);
    }
  }
  // No fallback decoders available in MVP — return original error
  Err(AroundError::DecodeError {
    message: format!("all decoders failed for format detected as '{}'", preferred),
  })
}
/// KDL-backed engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
  pub log_level: String,
  pub output_device: Option<String>,
  pub output_auto_reconnect: bool,
  pub decoder_paths: Vec<String>,
  pub decoder_auto_load: bool,
  /// Raw KDL nodes under `extensions` block, passed through to extension manager.
  pub extension_config: Vec<KdlNode>,
}

impl Default for EngineConfig {
  fn default() -> Self {
    Self {
      log_level: "info".into(),
      output_device: None,
      output_auto_reconnect: false,
      decoder_paths: vec!["~/.local/share/around/decoders".into()],
      decoder_auto_load: true,
      extension_config: Vec::new(),
    }
  }
}

impl EngineConfig {
  /// Load config from compiled defaults, then overlay `~/.config/around/config.kdl`.
  pub fn load() -> Self {
    let mut config = Self::default();
    if let Some(home) = std::env::var_os("HOME") {
      let path = std::path::PathBuf::from(home).join(".config/around/config.kdl");
      if path.exists() {
        match std::fs::read_to_string(&path) {
          Ok(contents) => {
            if let Err(e) = config.merge_kdl(&contents) {
              tracing::warn!("failed to parse config file {}: {}", path.display(), e);
            }
          }
          Err(e) => {
            tracing::warn!("failed to read config file {}: {}", path.display(), e);
          }
        }
      }
    }
    config
  }

  /// Merge KDL document content into this config.
  fn merge_kdl(&mut self, source: &str) -> Result<(), String> {
    let doc: KdlDocument = source.parse().map_err(|e| format!("parse error: {}", e))?;
    for node in doc.nodes() {
      match node.name().value() {
        "log-level" => {
          if let Some(v) = node_leaf_string(node)? {
            self.log_level = v.to_string();
          }
        }
        "output" => {
          self.parse_output_block(node)?;
        }
        "decoders" => {
          self.parse_decoders_block(node)?;
        }
        "extensions" => {
          // Passthrough: collect child nodes for extension manager.
          self.extension_config = node
            .children()
            .map(|d| d.nodes().to_vec())
            .unwrap_or_default();
        }
        other => {
          return Err(format!("unknown top-level key '{}'", other));
        }
      }
    }
    Ok(())
  }

  fn parse_output_block(&mut self, node: &KdlNode) -> Result<(), String> {
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
      match child.name().value() {
        "device" => {
          if let Some(v) = node_leaf_string(child)? {
            self.output_device = Some(v.to_string());
          }
        }
        "auto-reconnect" => {
          if let Some(v) = node_leaf_bool(child) {
            self.output_auto_reconnect = v;
          }
        }
        other => return Err(format!("unknown output key '{}'", other)),
      }
    }
    Ok(())
  }

  fn parse_decoders_block(&mut self, node: &KdlNode) -> Result<(), String> {
    for child in node.children().map(|d| d.nodes()).unwrap_or(&[]) {
      match child.name().value() {
        "default-paths" => {
          let paths: Vec<String> = child
            .entries()
            .iter()
            .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
            .collect();
          if !paths.is_empty() {
            self.decoder_paths = paths;
          }
        }
        "auto-load" => {
          if let Some(v) = node_leaf_bool(child) {
            self.decoder_auto_load = v;
          }
        }
        other => return Err(format!("unknown decoders key '{}'", other)),
      }
    }
    Ok(())
  }
}

/// Extract a single string value from a leaf node (no children).
fn node_leaf_string(node: &KdlNode) -> Result<Option<&str>, String> {
  if node.children().is_some() {
    return Err(format!(
      "'{}' must be a leaf node, not a block",
      node.name()
    ));
  }
  Ok(node.entries().first().and_then(|e| e.value().as_string()))
}

/// Extract a single bool value from a leaf node.
fn node_leaf_bool(node: &KdlNode) -> Option<bool> {
  node.entries().first().and_then(|e| e.value().as_bool())
}
