//! C ABI export surface for the extension framework (ADR-0004 §5.5).
//!
//! Each function validates inputs (null checks), converts C string slices to
//! Rust `&str`, delegates to [`Framework`] methods, and maps errors to return
//! values (null pointer / false).
//!
//! # Safety
//!
//! All functions are `unsafe` because they accept raw pointers from C callers.
//! Callers must ensure:
//! - Pointer parameters are valid, aligned, and non-null (unless documented
//!   otherwise).
//! - `*const u8` slices have at least `len` readable bytes.
//! - Returned pointers are valid until the next call to the same function or
//!   until the referenced extension is unloaded.

use crate::{ExtensionMeta, Framework, IndexedMeta, PendingReload, RegisterVTable};
use libloading::Symbol;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert `*const u8 + len` to `&str`. Returns `None` on null pointer or
/// invalid UTF-8.
fn c_str_to_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
  if ptr.is_null() {
    return None;
  }
  let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
  std::str::from_utf8(bytes).ok()
}

/// Get a stable meta pointer for a loaded extension by name.
fn meta_ptr_by_name(name: &str) -> Option<*const ExtensionMeta> {
  let fw = Framework::instance();
  let loaded_meta = fw.loaded_meta.read();
  loaded_meta
    .get(name)
    .map(|boxed| &boxed.meta as *const ExtensionMeta)
}

// ---------------------------------------------------------------------------
// Counter for temp-file naming in `around_load_bytes`
// ---------------------------------------------------------------------------
static LOAD_BYTES_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write `data` to a uniquely-named temp file and return the path.
fn write_temp_so(data: &[u8]) -> Option<std::path::PathBuf> {
  let count = LOAD_BYTES_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  let dir = std::env::temp_dir().join("around-extensions-load");
  std::fs::create_dir_all(&dir).ok()?;
  let path = dir.join(format!("load_bytes_{count:x}.so"));
  std::fs::write(&path, data).ok()?;
  Some(path)
}

// ---------------------------------------------------------------------------
// Wrapper to make `*const ExtensionMeta` Send+Sync for the static cache.
// ---------------------------------------------------------------------------

/// Safe wrapper around a raw meta pointer for use in the static dep cache.
#[derive(Clone, Copy)]
#[repr(transparent)]
struct MetaPtr {
  ptr: *const ExtensionMeta,
}
// SAFETY: The pointer is only accessed under the DEPS_CACHE mutex.
unsafe impl Send for MetaPtr {}
unsafe impl Sync for MetaPtr {}

// ---------------------------------------------------------------------------
// Cache for `around_dependencies_of`
// ---------------------------------------------------------------------------
static DEPS_CACHE: LazyLock<Mutex<Vec<MetaPtr>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// C-compatible return type for `around_dependencies_of`.
///
/// Layout matches `stabby::Slice<'static, *const ExtensionMeta>`:
/// `{ data: *const T, len: usize }` (both `#[repr(C)]`).
#[repr(C)]
pub struct ExtensionMetaSlice {
  /// Pointer to the first element of the slice.
  pub data: *const *const ExtensionMeta,
  /// Number of elements in the slice.
  pub len: usize,
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Load an extension from a file path.
///
/// Returns a pointer to the loaded extension's [`ExtensionMeta`], or null on
/// error.
#[no_mangle]
pub unsafe extern "C" fn around_load(path: *const u8, path_len: usize) -> *const ExtensionMeta {
  let path_str = match c_str_to_str(path, path_len) {
    Some(s) => s,
    None => return std::ptr::null(),
  };
  let path = Path::new(path_str);
  let fw = Framework::instance();

  // dlopen to read AROUND_META and add to index, then delegate to load().
  let meta_copy = match (|| -> Option<IndexedMeta> {
    let lib = unsafe { crate::open_library(path) }.ok()?;
    let meta_sym: Symbol<&'static ExtensionMeta> = unsafe { lib.get(b"AROUND_META\0") }.ok()?;
    Some(IndexedMeta::from_ffi(*meta_sym))
  })() {
    Some(m) => m,
    None => return std::ptr::null(),
  };
  // lib dropped here (dlclose). IndexedMeta owns cloned data.

  let name = meta_copy.name.clone();
  {
    let mut index = fw.index.write();
    if !index.contains_key(&name) {
      index.insert(name.clone(), (path.to_path_buf(), meta_copy));
    }
  }

  match fw.load(&name) {
    Ok(_) => meta_ptr_by_name(&name).unwrap_or(std::ptr::null()),
    Err(_) => std::ptr::null(),
  }
}

/// Load an extension from in-memory bytes.
///
/// The bytes are written to a temporary file, then loaded normally. Returns a
/// pointer to the loaded extension's [`ExtensionMeta`], or null on error.
#[no_mangle]
pub unsafe extern "C" fn around_load_bytes(
  data: *const u8,
  data_len: usize,
) -> *const ExtensionMeta {
  if data.is_null() || data_len == 0 {
    return std::ptr::null();
  }
  let bytes = unsafe { std::slice::from_raw_parts(data, data_len) };
  let fw = Framework::instance();

  let tmp_path = match write_temp_so(bytes) {
    Some(p) => p,
    None => return std::ptr::null(),
  };

  let meta_copy = match (|| -> Option<IndexedMeta> {
    let lib = unsafe { crate::open_library(&tmp_path) }.ok()?;
    let meta_sym: Symbol<&'static ExtensionMeta> = unsafe { lib.get(b"AROUND_META\0") }.ok()?;
    Some(IndexedMeta::from_ffi(*meta_sym))
  })() {
    Some(m) => m,
    None => {
      let _ = std::fs::remove_file(&tmp_path);
      return std::ptr::null();
    }
  };

  let name = meta_copy.name.clone();
  {
    let mut index = fw.index.write();
    if !index.contains_key(&name) {
      index.insert(name.clone(), (tmp_path, meta_copy));
    }
  }

  match fw.load(&name) {
    Ok(_) => meta_ptr_by_name(&name).unwrap_or(std::ptr::null()),
    Err(_) => std::ptr::null(),
  }
}

/// Unload an extension identified by its [`ExtensionMeta`] pointer.
///
/// Returns `true` on success, `false` on error (extension not found, or
/// dependents still loaded).
#[no_mangle]
pub unsafe extern "C" fn around_unload(meta: *const ExtensionMeta) -> bool {
  if meta.is_null() {
    return false;
  }

  let fw = Framework::instance();

  let name = {
    let loaded_meta = fw.loaded_meta.read();
    let mut found: Option<Box<str>> = None;
    for (n, boxed) in loaded_meta.iter() {
      if std::ptr::eq(&boxed.meta, meta) {
        found = Some(n.clone());
        break;
      }
    }
    match found {
      Some(n) => n,
      None => return false,
    }
  };

  fw.unload(&name).is_ok()
}

/// Phase 1 of the two-phase hot-reload protocol.
///
/// Loads a new extension `.so` from `path` and returns an opaque handle
/// (`*mut c_void`) for use with [`around_commit_reload`]. Returns null on
/// error.
#[no_mangle]
pub unsafe extern "C" fn around_prepare_reload(
  path: *const u8,
  path_len: usize,
) -> *mut std::ffi::c_void {
  let path_str = match c_str_to_str(path, path_len) {
    Some(s) => s,
    None => return std::ptr::null_mut(),
  };
  let path = Path::new(path_str);

  match Framework::instance().prepare_reload(path) {
    Ok(pending) => Box::into_raw(Box::new(pending)) as *mut std::ffi::c_void,
    Err(_) => std::ptr::null_mut(),
  }
}

/// Phase 2 of the two-phase hot-reload protocol.
///
/// Consumes the opaque handle from [`around_prepare_reload`] and atomically
/// replaces the extension identified by `old_meta`. Returns `true` on success.
#[no_mangle]
pub unsafe extern "C" fn around_commit_reload(
  old_meta: *const ExtensionMeta,
  pending: *mut std::ffi::c_void,
) -> bool {
  if old_meta.is_null() || pending.is_null() {
    return false;
  }

  let pending_box = unsafe { Box::from_raw(pending as *mut PendingReload) };
  Framework::instance()
    .commit_reload(old_meta, *pending_box)
    .is_ok()
}

// ---------------------------------------------------------------------------
// Slot management
// ---------------------------------------------------------------------------

/// Register a new Slot's Register with the framework.
///
/// `slot_name` is the canonical slot name (e.g. `"around_audio_sdk::Codec"`).
/// `vtable` points to a [`RegisterVTable`] struct and `reg_instance` is the
/// opaque Register handle.
#[no_mangle]
pub unsafe extern "C" fn around_attach_register(
  slot_name: *const u8,
  slot_name_len: usize,
  vtable: *const (),
  reg_instance: *mut (),
) {
  let name = match c_str_to_str(slot_name, slot_name_len) {
    Some(s) => s,
    None => return,
  };
  if vtable.is_null() || reg_instance.is_null() {
    return;
  }

  // SAFETY: caller guarantees the vtable and register instance are valid
  // and live for the program's lifetime.
  let vt: &'static RegisterVTable = unsafe { &*(vtable as *const RegisterVTable) };
  Framework::instance().attach_register(name, vt, reg_instance);
}

/// Push an entry into a registered slot.
///
/// `slot_name`, `vtable`, and `entry` identify the slot and the entry to add.
/// `source` points to the [`ExtensionMeta`] of the extension providing the
/// entry.
#[no_mangle]
pub unsafe extern "C" fn around_push_entry(
  slot_name: *const u8,
  slot_name_len: usize,
  vtable: *const (),
  entry: *mut (),
  source: *const ExtensionMeta,
) {
  let name = match c_str_to_str(slot_name, slot_name_len) {
    Some(s) => s,
    None => return,
  };
  if vtable.is_null() || entry.is_null() || source.is_null() {
    return;
  }

  let fw = Framework::instance();
  let slot_map = fw.slot_map.read();
  if let Some(storage) = slot_map.get(name) {
    let (vt, opaque_reg) = match storage {
      crate::SlotStorage::Static { vtable, reg } => (*vtable, *reg),
      crate::SlotStorage::Owned { vtable, reg, .. } => (*vtable, *reg),
    };
    // SAFETY: opaque_reg is the Register pointer from attach_register.
    unsafe { (vt.push_raw)(opaque_reg.0, entry, source) };
  }
}

// ---------------------------------------------------------------------------
// Dependency graph
// ---------------------------------------------------------------------------

/// Return the list of loaded dependencies as a slice of `*const ExtensionMeta`
/// pointers.
///
/// The returned [`ExtensionMetaSlice`] is valid until the next call to this
/// function. Callers that need to retain the data must copy it before calling
/// again.
#[no_mangle]
pub unsafe extern "C" fn around_dependencies_of(meta: *const ExtensionMeta) -> ExtensionMetaSlice {
  let meta_ref = match unsafe { meta.as_ref() } {
    Some(m) => m,
    None => {
      return ExtensionMetaSlice {
        data: std::ptr::null(),
        len: 0,
      };
    }
  };

  let dep_names = meta_ref.depends_on();
  let fw = Framework::instance();

  let mut cache = match DEPS_CACHE.lock() {
    Ok(g) => g,
    Err(_) => {
      return ExtensionMetaSlice {
        data: std::ptr::null(),
        len: 0,
      };
    }
  };
  cache.clear();

  for dep_name in &dep_names {
    let loaded_meta = fw.loaded_meta.read();
    if let Some(boxed) = loaded_meta.get(*dep_name) {
      cache.push(MetaPtr {
        ptr: &boxed.meta as *const ExtensionMeta,
      });
    }
  }

  if cache.is_empty() {
    ExtensionMetaSlice {
      data: std::ptr::null(),
      len: 0,
    }
  } else {
    // SAFETY: cache is non-empty, so the pointer is valid.
    // The data lives in the static DEPS_CACHE until the next call.
    let ptr = cache.as_ptr() as *const *const ExtensionMeta;
    ExtensionMetaSlice {
      data: ptr,
      len: cache.len(),
    }
  }
}

/// Check whether an extension is currently loaded.
#[no_mangle]
pub unsafe extern "C" fn around_is_loaded(name: *const u8, name_len: usize) -> bool {
  match c_str_to_str(name, name_len) {
    Some(s) => Framework::instance().is_loaded(s),
    None => false,
  }
}
