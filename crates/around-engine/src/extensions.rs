//! Extension manager: runtime loading/unloading of decoder shared libraries.
//!
//! Extensions are `.so`/`.dylib`/`.dll` files that export a single FFI entry point:
//! `fn create_decoder() -> Box<ErasedDecoder>`.
//!
//! The [`ExtensionManager`] owns the loaded libraries — they stay alive as long as
//! they are registered. Callers receive lightweight [`DecoderInfo`] snapshots.

use around_core::{AroundError, ErasedDecoder, FormatSignature};
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// FFI entry point signature for decoder extensions.
type CreateDecoderFn = unsafe fn() -> Box<ErasedDecoder>;

/// Metadata about a loaded decoder, safe to clone and share.
#[derive(Debug, Clone)]
pub struct DecoderInfo {
  pub name: String,
  pub formats: Vec<String>,
  pub source: String,
  pub path: Option<PathBuf>,
}

/// Internal entry: holds both the live [`Library`] and its metadata.
struct DecoderEntry {
  _library: Library,
  info: DecoderInfo,
}

/// Registry of loaded decoder extensions.
///
/// Thread-safe: all methods take `&self` and synchronise internally.
pub struct ExtensionManager {
  entries: Mutex<HashMap<String, DecoderEntry>>,
}

impl Default for ExtensionManager {
  fn default() -> Self {
    Self::new()
  }
}

impl ExtensionManager {
  pub fn new() -> Self {
    Self {
      entries: Mutex::new(HashMap::new()),
    }
  }

  /// Load a decoder from a shared library file at `path`.
  ///
  /// The file must export `create_decoder` with the [`CreateDecoderFn`] signature.
  /// The decoder's name is derived from the file stem. Returns an error if a
  /// decoder with the same name is already registered.
  pub fn load_path(&self, path: &Path) -> Result<DecoderInfo, AroundError> {
    let lib = unsafe {
      Library::new(path).map_err(|e| AroundError::DecoderLoadFailed {
        path: Some(path.display().to_string()),
        reason: format!("failed to load library: {}", e),
      })?
    };

    let create: Symbol<CreateDecoderFn> = unsafe {
      lib
        .get(b"create_decoder")
        .map_err(|e| AroundError::DecoderLoadFailed {
          path: Some(path.display().to_string()),
          reason: format!("symbol 'create_decoder' not found: {}", e),
        })?
    };

    let erased = unsafe { create() };
    let formats: Vec<String> = erased
      .supported_formats
      .iter()
      .filter_map(|f: &FormatSignature| f.extension.map(|s: &str| s.to_string()))
      .collect();

    let name = path
      .file_stem()
      .and_then(|s| s.to_str())
      .unwrap_or("unknown")
      .to_string();

    let mut entries = self.entries.lock().unwrap();
    if entries.contains_key(&name) {
      return Err(AroundError::DecoderLoadFailed {
        path: Some(path.display().to_string()),
        reason: format!("decoder '{}' already loaded", name),
      });
    }

    let info = DecoderInfo {
      name: name.clone(),
      formats: formats.clone(),
      source: "runtime".into(),
      path: Some(path.to_path_buf()),
    };

    entries.insert(
      name,
      DecoderEntry {
        _library: lib,
        info: info.clone(),
      },
    );
    Ok(info)
  }

  /// Load a decoder from base64-encoded shared library bytes.
  ///
  /// The bytes are written to a temporary file, loaded via [`load_path`],
  /// and the temp file is deleted immediately afterward.
  pub fn load_bytes(&self, data: &[u8], name: &str) -> Result<DecoderInfo, AroundError> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
      .decode(data)
      .map_err(|e| AroundError::DecoderLoadFailed {
        path: None,
        reason: format!("base64 decode failed: {}", e),
      })?;

    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("around_decoder_{}.so", name));
    std::fs::write(&tmp_path, &decoded).map_err(|e| AroundError::DecoderLoadFailed {
      path: Some(tmp_path.display().to_string()),
      reason: format!("failed to write temp file: {}", e),
    })?;

    let result = self.load_path(&tmp_path);
    let _ = std::fs::remove_file(&tmp_path);

    match result {
      Ok(mut info) => {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get_mut(&info.name) {
          entry.info.source = "bytes".into();
          entry.info.path = None;
        }
        info.source = "bytes".into();
        info.path = None;
        Ok(info)
      }
      Err(e) => Err(e),
    }
  }

  /// Scan `search_paths` directories for shared libraries and load them.
  ///
  /// Only files with `.so`, `.dylib`, or `.dll` extensions are considered.
  /// Load failures are silently skipped.
  pub fn discover(&self, search_paths: &[PathBuf]) -> Vec<DecoderInfo> {
    let mut discovered = Vec::new();
    for dir in search_paths {
      if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
          let path = entry.path();
          if path
            .extension()
            .is_some_and(|e| e == "so" || e == "dylib" || e == "dll")
          {
            if let Ok(mut info) = self.load_path(&path) {
              let mut entries = self.entries.lock().unwrap();
              if let Some(entry) = entries.get_mut(&info.name) {
                entry.info.source = "discovered".into();
              }
              info.source = "discovered".into();
              discovered.push(info);
            }
          }
        }
      }
    }
    discovered
  }

  /// Return a snapshot of all currently registered decoders.
  pub fn list_decoders(&self) -> Vec<DecoderInfo> {
    self
      .entries
      .lock()
      .unwrap()
      .values()
      .map(|e| e.info.clone())
      .collect()
  }
}
