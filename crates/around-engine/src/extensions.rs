//! Extension manager: runtime loading/unloading of codec shared libraries.
//!
//! Extensions are `.so`/`.dylib`/`.dll` files that export a single FFI entry point:
//! `fn get_codec_info() -> *const CodecInfoFFI`.
//!
//! The [`ExtensionManager`] owns the loaded libraries — they stay alive as long as
//! they are registered. Callers receive lightweight [`DecoderInfo`] snapshots.

use around_core::AroundError;
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// FFI types
// ---------------------------------------------------------------------------

/// C-ABI codec descriptor returned by `get_codec_info()`.
#[repr(C)]
pub struct CodecInfoFFI {
  pub name_ptr: *const u8,
  pub name_len: usize,
  pub extensions_ptr: *const u8,
  pub extensions_len: usize,
}

/// FFI entry point signature for codec extensions.
type GetCodecInfoFn = unsafe fn() -> *const CodecInfoFFI;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Metadata about a loaded codec, safe to clone and share.
#[derive(Debug, Clone)]
pub struct DecoderInfo {
  pub name: String,
  pub formats: Vec<String>,
  pub source: String,
  pub path: Option<PathBuf>,
}

/// Internal entry: holds the live [`Library`] and its [`CodecInfo`].
struct CodecEntry {
  _library: Library,
  info: DecoderInfo,
}

/// Registry of loaded codec extensions.
pub struct ExtensionManager {
  entries: Mutex<HashMap<String, CodecEntry>>,
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

  /// Load a codec from a shared library file at `path`.
  pub fn load_path(&self, path: &Path) -> Result<DecoderInfo, AroundError> {
    let lib = unsafe {
      Library::new(path).map_err(|e| AroundError::DecoderLoadFailed {
        path: Some(path.display().to_string()),
        reason: format!("failed to load library: {}", e),
      })?
    };

    let get_info: Symbol<GetCodecInfoFn> = unsafe {
      lib
        .get(b"get_codec_info")
        .map_err(|e| AroundError::DecoderLoadFailed {
          path: Some(path.display().to_string()),
          reason: format!("symbol 'get_codec_info' not found: {}", e),
        })?
    };

    let ffi = unsafe { get_info() };
    if ffi.is_null() {
      return Err(AroundError::DecoderLoadFailed {
        path: Some(path.display().to_string()),
        reason: "get_codec_info returned null".into(),
      });
    }

    let ffi_ref = unsafe { &*ffi };
    let name = unsafe {
      std::str::from_utf8(std::slice::from_raw_parts(
        ffi_ref.name_ptr,
        ffi_ref.name_len,
      ))
      .unwrap_or("unknown")
    }
    .to_string();

    let ext_str = unsafe {
      std::str::from_utf8(std::slice::from_raw_parts(
        ffi_ref.extensions_ptr,
        ffi_ref.extensions_len,
      ))
      .unwrap_or("")
    };

    let formats: Vec<String> = ext_str
      .split('\0')
      .filter(|s| !s.is_empty())
      .map(|s| s.to_string())
      .collect();

    let mut entries = self.entries.lock().unwrap();
    if entries.contains_key(&name) {
      return Err(AroundError::DecoderLoadFailed {
        path: Some(path.display().to_string()),
        reason: format!("codec '{}' already loaded", name),
      });
    }

    let info = DecoderInfo {
      name: name.clone(),
      formats: formats.clone(),
      source: "runtime".into(),
      path: Some(path.to_path_buf()),
    };

    entries.insert(
      name.clone(),
      CodecEntry {
        _library: lib,
        info: info.clone(),
      },
    );

    Ok(info)
  }

  /// Load a codec from base64-encoded shared library bytes.
  pub fn load_bytes(&self, data: &[u8], name: &str) -> Result<DecoderInfo, AroundError> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
      .decode(data)
      .map_err(|e| AroundError::DecoderLoadFailed {
        path: None,
        reason: format!("base64 decode failed: {}", e),
      })?;

    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!(
      "around_decoder_{}.{}",
      name,
      std::env::consts::DLL_EXTENSION
    ));
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

  /// Return a snapshot of all currently registered codecs.
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
