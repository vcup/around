//! Engine configuration.
use kdl::KdlDocument;
use std::path::PathBuf;

/// Selects the output Adapter for decode loops.
///
/// * `Cpal` — native output via the CPAL Adapter (default)
/// * `Null` — deterministic in-memory discard (headless/CI)
#[derive(Debug, Clone)]
pub enum OutputDriver {
  /// Native output discovered through the CPAL Adapter.
  Cpal,
  /// Deterministic discard through the in-memory Adapter.
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
          std::fs::read_to_string(path).map_err(|e| format!("failed to read {path:?}: {e}"))?;
        return Self::parse(&content);
      }
    }
    Ok(Self::default())
  }

  fn parse(input: &str) -> Result<Self, String> {
    let doc: KdlDocument = input.parse().map_err(|e| format!("KDL parse error: {e}"))?;
    let mut cfg = Self::default();
    for node in doc.nodes() {
      if node.name().value() == "output" {
        for entry in node.entries() {
          match entry.name().map(|n| n.value()) {
            Some("device") => cfg.output_device = entry.value().as_string().map(String::from),
            Some("auto_reconnect") => {
              cfg.output_auto_reconnect = entry.value().as_bool().unwrap_or(true)
            }
            Some("driver") => {
              cfg.output_driver = match entry.value().as_string() {
                Some("null") => OutputDriver::Null,
                Some("cpal") => OutputDriver::Cpal,
                Some(other) => return Err(format!("unknown output driver: {other}")),
                None => return Err("output driver must be a string".into()),
              };
            }
            _ => {}
          }
        }
      }
    }
    Ok(cfg)
  }
}

#[cfg(test)]
mod tests {
  #![expect(
    clippy::unwrap_used,
    reason = "configuration parser fixtures are fixed valid KDL"
  )]
  use super::*;

  #[test]
  fn parses_output_driver() {
    let config = EngineConfig::parse("output driver=\"null\"").unwrap();
    assert!(matches!(config.output_driver, OutputDriver::Null));
  }

  #[test]
  fn rejects_unknown_output_driver() {
    assert!(EngineConfig::parse("output driver=\"alsa\"").is_err());
  }
}
