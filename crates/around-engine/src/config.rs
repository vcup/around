//! Engine configuration.
use kdl::KdlDocument;
use std::path::PathBuf;

/// Selects the audio output driver for decode loops.
///
/// * `Cpal` — real audio device via cpal (default, requires audio hardware)
/// * `Null` — silent discard (headless/CI)
///
/// Custom sinks can be used directly with
/// [`Engine::run_stream_with_sink`](crate::pipeline::Engine::run_stream_with_sink).
#[derive(Debug, Clone)]
pub enum OutputDriver {
  /// Real audio output via the default cpal device.
  Cpal,
  /// Silent discard — no audio hardware needed.
  Null,
}

/// KDL-backed engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
  pub output_device: Option<String>,
  pub output_auto_reconnect: bool,
  pub codec_search_paths: Vec<PathBuf>,
  /// Selects the audio output backend. Defaults to `Cpal`.
  pub output_driver: OutputDriver,
}

impl Default for EngineConfig {
  fn default() -> Self {
    Self {
      output_device: None,
      output_auto_reconnect: true,
      codec_search_paths: Vec::new(),
      output_driver: OutputDriver::Cpal,
    }
  }
}

impl EngineConfig {
  /// Load with defaults — no config file search.
  pub fn load() -> Self {
    Self::default()
  }

  /// Load configuration from the first existing path in `candidates`.
  /// Returns defaults if no config file is found.
  pub fn load_from(candidates: &[impl AsRef<std::path::Path>]) -> Result<Self, String> {
    for p in candidates {
      let path = p.as_ref();
      if path.exists() {
        let content =
          std::fs::read_to_string(path).map_err(|e| format!("failed to read {:?}: {}", path, e))?;
        return Self::parse(&content);
      }
    }
    Ok(Self::default())
  }

  fn parse(input: &str) -> Result<Self, String> {
    let doc: KdlDocument = input
      .parse()
      .map_err(|e| format!("KDL parse error: {}", e))?;
    let mut cfg = Self::default();

    for node in doc.nodes() {
      if node.name().value() == "output" {
        for entry in node.entries() {
          match entry.name().map(|n| n.value()) {
            Some("device") => cfg.output_device = entry.value().as_string().map(String::from),
            Some("auto_reconnect") => {
              cfg.output_auto_reconnect = entry.value().as_bool().unwrap_or(true)
            }
            _ => {}
          }
        }
      }
    }
    Ok(cfg)
  }
}
