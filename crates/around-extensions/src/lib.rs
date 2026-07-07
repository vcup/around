//! around-extensions: Host-agnostic extension loading framework.
//!
//! Every `.so` exporting `AROUND_META` is an Extension. This crate provides
//! discovery, dependency resolution, lifecycle management, and progressive-trust
//! crash isolation. The framework knows no concrete Slot types — it only sees
//! [`RegisterVTable`].
//!
//! # Architecture
//!
//! - [`ExtensionMeta`] — ABI-stable descriptor exported via `AROUND_META` symbol.
//! - [`RegisterVTable`] — opaque vtable for slot operations (push_raw, remove_by_meta).
//! - [`Framework`] — global singleton managing scanned index, loaded libraries, and active slots.

#![deny(unsafe_op_in_unsafe_fn)]
use crash_guard::{crash_guard_unsafe, CrashError};
use libloading::{Library, Symbol};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
mod cabi;
pub mod crash_guard;
mod lifecycle;
mod trust;
use lifecycle::LifecycleRegister;
use trust::TrustCounter;

pub const CURRENT_API_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Platform abstraction + helpers
// ---------------------------------------------------------------------------

/// Platform-gated shared library extension filter.
#[cfg(target_os = "linux")]
const SHARED_LIB_EXTENSIONS: &[&str] = &["so"];
#[cfg(target_os = "macos")]
const SHARED_LIB_EXTENSIONS: &[&str] = &["dylib", "so"];
#[cfg(target_os = "windows")]
const SHARED_LIB_EXTENSIONS: &[&str] = &["dll"];

/// Open a shared library with RTLD_NOW | RTLD_LOCAL on Unix.
///
/// # Safety
/// Same as `Library::new` — the caller must ensure the path points to a
/// valid shared library and that no undefined-behaviour-inducing operations
/// are performed through the returned handle.
#[cfg(unix)]
unsafe fn open_library(path: &std::path::Path) -> Result<Library, libloading::Error> {
  // RTLD_LOCAL: per ADR-0004 §11, extensions must not export symbols to the global table.
  let flags = libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_LOCAL;
  unsafe { libloading::os::unix::Library::open(Some(path), flags) }.map(Library::from)
}

#[cfg(not(unix))]
unsafe fn open_library(path: &std::path::Path) -> Result<Library, libloading::Error> {
  // SAFETY: caller guarantees path is valid
  unsafe { Library::new(path) }
}

// ---------------------------------------------------------------------------
// ABI-stable extension metadata (repr(C), no stabby dependency needed)
// ---------------------------------------------------------------------------

/// ABI-stable descriptor exported by every extension `.so` via the
/// `AROUND_META` symbol.  Uses `#[repr(C)]` with raw-pointer fields for
/// maximum FFI compatibility (stabby 72 does not support `str` or `[T]`
/// as `IStable` types).
#[repr(C)]
pub struct ExtensionMeta {
  /// Extension name (e.g. `"around-codec-wav"`), null-terminated C string.
  pub name: *const u8,
  /// Semantic version string, null-terminated.
  pub semver: *const u8,
  /// Bumped when the extension's ABI contract changes.
  pub api_version: u32,
  /// _reserved padding for alignment (api_version is only 4 bytes).
  pub _pad: u32,
  /// Extension names this one depends on (must be loaded first).
  /// Pointer to an array of null-terminated C strings, terminated by a null pointer.
  pub depends_on: *const *const u8,
  /// Slot names this extension requires to be registered before it can load.
  /// Same format as depends_on.
  pub depends_on_slots: *const *const u8,
  /// Extension author identifier, null-terminated.
  pub author: *const u8,
  /// Human-readable description, null-terminated.
  pub description: *const u8,
}

// SAFETY: ExtensionMeta contains only static pointers read from a .so's
// data segment. It is never mutated at runtime.
unsafe impl Send for ExtensionMeta {}
unsafe impl Sync for ExtensionMeta {}

impl ExtensionMeta {
  /// Read a null-terminated C string from a `*const u8` pointer.
  ///
  /// # Safety
  /// `ptr` must point to a valid null-terminated UTF-8 string.
  unsafe fn read_str(ptr: *const u8, what: &'static str) -> &'static str {
    if ptr.is_null() {
      return "";
    }
    let mut len: usize = 0;
    while unsafe { *ptr.add(len) } != 0 {
      len += 1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    match std::str::from_utf8(bytes) {
      Ok(s) => s,
      Err(_) => {
        tracing::warn!(field = what, "invalid UTF-8 in extension metadata");
        ""
      }
    }
  }

  /// Read a null-terminated array of C strings from `*const *const u8`.
  ///
  /// # Safety
  /// `ptr` must point to a valid null-terminated array of C strings.
  unsafe fn read_str_array(ptr: *const *const u8, what: &'static str) -> Vec<&'static str> {
    if ptr.is_null() {
      return Vec::new();
    }
    let mut result = Vec::new();
    let mut i: usize = 0;
    loop {
      let s = unsafe { *ptr.add(i) };
      if s.is_null() {
        break;
      }
      result.push(unsafe { Self::read_str(s, what) });
      i += 1;
    }
    result
  }

  /// Safe accessor for the name field.
  pub fn name(&self) -> &'static str {
    unsafe { Self::read_str(self.name, "name") }
  }

  /// Safe accessor for the semver field.
  pub fn semver(&self) -> &'static str {
    unsafe { Self::read_str(self.semver, "semver") }
  }

  /// Safe accessor for the author field.
  pub fn author(&self) -> &'static str {
    unsafe { Self::read_str(self.author, "author") }
  }

  /// Safe accessor for the description field.
  pub fn description(&self) -> &'static str {
    unsafe { Self::read_str(self.description, "description") }
  }

  /// Safe accessor for depends_on.
  pub fn depends_on(&self) -> Vec<&'static str> {
    unsafe { Self::read_str_array(self.depends_on, "depends_on") }
  }

  /// Safe accessor for depends_on_slots.
  pub fn depends_on_slots(&self) -> Vec<&'static str> {
    unsafe { Self::read_str_array(self.depends_on_slots, "depends_on_slots") }
  }
}

impl Clone for ExtensionMeta {
  fn clone(&self) -> Self {
    *self
  }
}

impl Copy for ExtensionMeta {}

// ---------------------------------------------------------------------------
// IndexedMeta — owned extension metadata for the index
// ---------------------------------------------------------------------------

/// Owned copy of extension metadata stored in the index, with all strings
/// heap-allocated.  Safe to keep after the .so is scanned and closed.
#[derive(Clone)]
struct IndexedMeta {
  name: Box<str>,
  semver: Box<str>,
  api_version: u32,
  depends_on: Vec<Box<str>>,
  depends_on_slots: Vec<Box<str>>,
  author: Box<str>,
  description: Box<str>,
}

impl IndexedMeta {
  fn from_ffi(meta: &ExtensionMeta) -> Self {
    Self {
      name: Box::from(meta.name()),
      semver: Box::from(meta.semver()),
      api_version: meta.api_version,
      depends_on: meta.depends_on().into_iter().map(Box::from).collect(),
      depends_on_slots: meta.depends_on_slots().into_iter().map(Box::from).collect(),
      author: Box::from(meta.author()),
      description: Box::from(meta.description()),
    }
  }

  fn depends_on(&self) -> &[Box<str>] {
    &self.depends_on
  }

  fn to_heap_ffi(&self) -> Box<OwnedFFIMeta> {
    use std::ffi::CString;

    fn nul_safe(s: &str, field: &str) -> CString {
      CString::new(s.as_bytes()).unwrap_or_else(|e| {
        tracing::warn!(
          field,
          "NUL byte at position {}, truncating",
          e.nul_position()
        );
        CString::new(&s.as_bytes()[..e.nul_position()])
          .unwrap_or_else(|_| unreachable!("truncated prefix of a NUL-free string"))
      })
    }

    let name = nul_safe(&self.name, "name");
    let semver = nul_safe(&self.semver, "semver");
    let author = nul_safe(&self.author, "author");
    let description = nul_safe(&self.description, "description");

    let deps_strs: Vec<CString> = self
      .depends_on
      .iter()
      .map(|s| nul_safe(s, "depends_on"))
      .collect();
    let mut dep_ptrs: Vec<*const u8> = deps_strs
      .iter()
      .map(|cs| cs.as_ptr() as *const u8)
      .collect();
    dep_ptrs.push(std::ptr::null());

    let slot_strs: Vec<CString> = self
      .depends_on_slots
      .iter()
      .map(|s| nul_safe(s, "depends_on_slots"))
      .collect();
    let mut slot_ptrs: Vec<*const u8> = slot_strs
      .iter()
      .map(|cs| cs.as_ptr() as *const u8)
      .collect();
    slot_ptrs.push(std::ptr::null());

    let meta = ExtensionMeta {
      name: name.as_ptr() as *const u8,
      semver: semver.as_ptr() as *const u8,
      api_version: self.api_version,
      _pad: 0,
      depends_on: dep_ptrs.as_ptr(),
      depends_on_slots: slot_ptrs.as_ptr(),
      author: author.as_ptr() as *const u8,
      description: description.as_ptr() as *const u8,
    };

    Box::new(OwnedFFIMeta {
      meta,
      _name: name,
      _semver: semver,
      _author: author,
      _description: description,
      _depends_on_ptrs: dep_ptrs,
      _depends_on_strs: deps_strs,
      _depends_on_slots_ptrs: slot_ptrs,
      _depends_on_slots_strs: slot_strs,
    })
  }
}

// ---------------------------------------------------------------------------
// OwnedFFIMeta — heap-stable ExtensionMeta with owned backing strings
// ---------------------------------------------------------------------------

/// A heap-allocated `ExtensionMeta` whose raw pointer fields are backed by
/// owned `CString`/`Vec` storage in the same struct.  Dropping this wrapper
/// frees all FFI-facing allocations.
///
/// The inner `meta: ExtensionMeta` has a stable address on the heap (wrapped
/// in a `Box`).  Slot operations receive `&meta as *const ExtensionMeta`,
/// which remains valid as long as this `OwnedFFIMeta` lives.
struct OwnedFFIMeta {
  meta: ExtensionMeta,
  _name: std::ffi::CString,
  _semver: std::ffi::CString,
  _author: std::ffi::CString,
  _description: std::ffi::CString,
  _depends_on_ptrs: Vec<*const u8>,
  _depends_on_strs: Vec<std::ffi::CString>,
  _depends_on_slots_ptrs: Vec<*const u8>,
  _depends_on_slots_strs: Vec<std::ffi::CString>,
}

// SAFETY: OwnedFFIMeta's raw pointers point into its own fields (CStrings, Vecs).
// The struct is self-contained and never mutated after construction.
unsafe impl Send for OwnedFFIMeta {}
unsafe impl Sync for OwnedFFIMeta {}
// SAFETY: Fields drop in declaration order (Rust's guarantee for structs).
// `meta` is declared first and drops first; the raw pointers it holds point
// into the CString/Vec fields declared after it, which drop later.
// This ordering is CORRECT: the backing strings must outlive `meta` because
// they back the raw pointers stored in `meta`.
impl Drop for OwnedFFIMeta {
  fn drop(&mut self) {
    // `meta` drops first (declaration order), invalidating its raw pointers.
    // The CString/Vec backing fields drop next — by then the pointers are
    // never read. This is the intended invariant.
  }
}
// ---------------------------------------------------------------------------
// RegisterVTable — opaque slot management
// ---------------------------------------------------------------------------

/// Opaque vtable for slot operations.  The Framework uses these function
/// pointers without knowing the concrete slot type.  Slots (created by
/// `#[slot]`) provide typed wrappers that cast internally.
#[repr(C)]
pub struct RegisterVTable {
  /// Push a raw entry into the register.  `reg` points to the Register
  /// allocation; `entry` is the opaque codec/filter/etc pointer.
  pub push_raw: unsafe extern "C" fn(reg: *mut (), entry: *mut (), ext_meta: *const ExtensionMeta),
  /// Remove all entries whose metadata matches `meta`.  Returns the count
  /// of removed entries.  `reg` points to the Register allocation.
  pub remove_by_meta: unsafe extern "C" fn(reg: *mut (), meta: *const ExtensionMeta) -> usize,
}

// ---------------------------------------------------------------------------
// OpaquePtr — Send+Sync wrapper for raw pointers stored in Framework
// ---------------------------------------------------------------------------

/// Wrapper around `*mut ()` that asserts Send+Sync.
/// The pointers stored here come from Register allocations that are themselves
/// Send+Sync (created by `#[slot]`-generated code). The Framework never
/// dereferences them — it only passes them back to the RegisterVTable functions.
#[derive(Clone, Copy)]
struct OpaquePtr(*mut ());

// SAFETY: OpaquePtr is only created from Register allocations that live for
// the duration of the program (static or leaked). The Framework never
// dereferences the pointer; it only passes it to push_raw/remove_by_meta.
unsafe impl Send for OpaquePtr {}
unsafe impl Sync for OpaquePtr {}

// ---------------------------------------------------------------------------
// SlotDef — slot definition from AROUND_SLOTS
// ---------------------------------------------------------------------------

/// A single slot definition found in `AROUND_SLOTS` of an extension `.so`.
///
/// Each slot describes a typed register that the extension provides entries
/// for (e.g. Codec, Filter). The Framework uses these to dynamically attach
/// new register types introduced by an extension.
#[repr(C)]
pub(crate) struct SlotDef {
  /// Canonical slot name, e.g. `"around_audio_sdk::codec::Codec"`.
  pub slot_name: &'static str,
  /// Pointer to the `RegisterVTable` for this slot.
  pub reg_vtable: *const (),
  /// Opaque pointer to the Register instance.
  pub reg_instance: *mut (),
}

// SAFETY: SlotDef holds only static references and raw pointers obtained
// from the extension's data segment. It is never mutated after creation.
unsafe impl Send for SlotDef {}
unsafe impl Sync for SlotDef {}

// ---------------------------------------------------------------------------
// PendingReload — opaque handle for the two-phase hot-reload protocol
// ---------------------------------------------------------------------------

/// Opaque handle returned by [`Framework::prepare_reload`] and consumed by
/// [`Framework::commit_reload`].
///
/// Buffers the new extension's state (library handle, metadata, slot entries)
/// so the old extension can continue serving requests during the prepare phase.
pub struct PendingReload {
  /// dlopen'd library of the new extension version.
  lib: Arc<Library>,
  /// Heap-allocated `ExtensionMeta` with owned backing strings.
  meta: Box<OwnedFFIMeta>,
  /// Slot definitions from `AROUND_SLOTS` in the new `.so`.
  #[expect(dead_code, reason = "reserved for future use by attach_register")]
  slot_defs: Vec<SlotDef>,
  /// Per-slot registration entries, indexed by slot name.
  /// Populated by polling `{slot_name}_create()` during prepare.
  entries: HashMap<Box<str>, Vec<*mut ()>>,
  /// Extension names that the new version depends on.
  #[expect(dead_code, reason = "reserved for future dependency validation")]
  depends_on: Vec<Box<str>>,
}

// ---------------------------------------------------------------------------
// Framework — global singleton
// ---------------------------------------------------------------------------

/// Global extension framework singleton.
///
/// The framework owns the scanned index (name → path + meta), the active
pub struct Framework {
  /// slot_name → (RegisterVTable reference, opaque Register pointer).
  slot_map: RwLock<HashMap<Box<str>, (&'static RegisterVTable, OpaquePtr)>>,
  /// extension name → (path on disk, owned metadata).
  index: RwLock<HashMap<Box<str>, (PathBuf, IndexedMeta)>>,
  /// Loaded libraries kept alive (Arc so callers can share ownership).
  loaded: RwLock<HashMap<Box<str>, Arc<Library>>>,
  /// Heap-allocated ExtensionMeta for each loaded extension.
  /// Provides stable pointers for Slot register operations.
  loaded_meta: RwLock<HashMap<Box<str>, Box<OwnedFFIMeta>>>,
  /// Guard against concurrent loads of the same extension.
  loading: Mutex<std::collections::HashSet<Box<str>>>,
  /// Lifecycle hooks registered for OnLoad event.
  on_load: Mutex<LifecycleRegister>,
  /// Lifecycle hooks registered for OnUnload event.
  on_unload: Mutex<LifecycleRegister>,
  /// Lifecycle hooks registered for OnReload event.
  on_reload: Mutex<LifecycleRegister>,
  /// Per-extension trust counters mapping extension name → counter.
  loaded_trust: RwLock<HashMap<Box<str>, TrustCounter>>,
  /// Per-Register trust counters keyed by slot name.
  /// Initialised in [`attach_register`] and updated by
  /// [`record_guarded_outcome`]. When a Register reaches `Trusted`,
  /// crash guard elision is eligible for that slot's operations.
  slot_trust: RwLock<HashMap<Box<str>, TrustCounter>>,
}
impl Framework {
  pub fn instance() -> &'static Framework {
    static FW: LazyLock<Framework> = LazyLock::new(|| Framework {
      slot_map: RwLock::new(HashMap::new()),
      index: RwLock::new(HashMap::new()),
      loaded: RwLock::new(HashMap::new()),
      loaded_meta: RwLock::new(HashMap::new()),
      loading: Mutex::new(std::collections::HashSet::new()),
      on_load: Mutex::new(LifecycleRegister::new()),
      on_unload: Mutex::new(LifecycleRegister::new()),
      on_reload: Mutex::new(LifecycleRegister::new()),
      loaded_trust: RwLock::new(HashMap::new()),
      slot_trust: RwLock::new(HashMap::new()),
    });
    &FW
  }

  /// Register a Slot's Register with the framework.
  ///
  /// Called by SDK init functions (e.g. `init_codec()`) at startup.
  /// `name` is the canonical slot name (e.g. `"Codec"`).
  /// `vt` and `reg` come from the `#[slot]`-generated `CodecRegister`.
  pub fn attach_register(&self, name: &'static str, vt: &'static RegisterVTable, reg: *mut ()) {
    let mut map = self.slot_map.write();
    map.insert(Box::from(name), (vt, OpaquePtr(reg)));
    // Initialise per-Register trust counter (default 100 successes to reach Trusted).
    let mut slot_trust = self.slot_trust.write();
    slot_trust
      .entry(Box::from(name))
      .or_insert_with(|| TrustCounter::new(100));
    tracing::debug!(slot = name, "slot registered");
  }

  /// Record a guarded operation outcome for a Register.
  ///
  /// `slot_name` is the canonical slot name (e.g. `"around_audio_sdk::codec::Codec"`).
  /// `success` is `true` if the operation completed without a crash.
  ///
  /// When a Register accumulates `required` consecutive successes (default 100),
  /// it graduates to `Trusted` and crash guard elision becomes eligible.
  pub fn record_guarded_outcome(&self, slot_name: &str, success: bool) {
    if let Some(counter) = self.slot_trust.read().get(slot_name) {
      counter.record_guarded_outcome(success);
    }
  }

  /// Returns `true` if the Register for `slot_name` has reached `Trusted` and
  /// crash guard elision is eligible.
  pub fn is_slot_trusted(&self, slot_name: &str) -> bool {
    self
      .slot_trust
      .read()
      .get(slot_name)
      .is_some_and(|c| c.is_trusted())
  }

  /// Scan search paths for extensions.
  ///
  /// For each `.so` file in `search_paths`: `dlopen`, `dlsym("AROUND_META")`,
  /// read the [`ExtensionMeta`], and `dlclose`.  Populates the internal index.
  /// Does NOT load dependencies — that happens on first [`load`](Self::load).
  pub fn scan(&self, search_paths: &[PathBuf]) -> Result<(), FrameworkError> {
    for search_path in search_paths {
      let entries = std::fs::read_dir(search_path)
        .map_err(|e| FrameworkError::Io(format!("read_dir {:?}: {}", search_path, e)))?;
      for entry in entries {
        let entry = match entry {
          Ok(e) => e,
          Err(e) => {
            tracing::warn!(?search_path, err = %e, "skipping unreadable directory entry during scan");
            continue;
          }
        };
        let path = entry.path();

        // Only consider shared libraries using platform-gated extension filter.
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
          continue;
        };
        if !SHARED_LIB_EXTENSIONS.contains(&ext) {
          continue;
        }

        // SAFETY: dlopen an untrusted .so — the extension runs in-process.
        // This is the progressive-trust model: scan only reads metadata.
        let lib = match unsafe { open_library(&path) } {
          Ok(lib) => lib,
          Err(e) => {
            tracing::warn!(?path, err = %e, "dlopen failed during scan");
            continue;
          }
        };

        // SAFETY: dlsym returns a raw pointer to static read-only data.
        let meta_sym: Symbol<&'static ExtensionMeta> = match unsafe { lib.get(b"AROUND_META\0") } {
          Ok(sym) => sym,
          Err(_) => {
            tracing::debug!(?path, "no AROUND_META symbol, skipping");
            continue;
          }
        };

        let indexed = IndexedMeta::from_ffi(*meta_sym);
        let name: Box<str> = indexed.name.clone();

        let mut index = self.index.write();
        if let Some(existing) = index.get(&name) {
          tracing::warn!(
              name = &*name,
              existing = %existing.0.display(),
              new = %path.display(),
              "duplicate extension name; ignoring newer"
          );
        } else {
          tracing::info!(
              name = &*name,
              path = %path.display(),
              "scanned extension"
          );
          index.insert(name, (path.clone(), indexed));
        }
        // Library dropped here → dlclose.  IndexedMeta owns its data, so this is safe.
      }
    }
    Ok(())
  }

  /// Load an extension by name.
  ///
  /// 1. Resolve the name in the index (error if unknown).
  /// 2. Recursively load all `depends_on` extensions first.
  /// 3. `dlopen` the library with `RTLD_NOW | RTLD_LOCAL`.
  /// 4. Call `around_init` if the symbol exists.
  /// 5. For each registered slot: poll `{slot_name}_create(0)`, `(1)`, ... until null.
  ///    Call `push_raw` for each non-null return.
  /// 6. Return an `Arc<Library>` that keeps the `.so` alive.
  ///
  /// # Slot name convention
  ///
  /// The create symbol name for a slot is derived as:
  /// `slot_name.to_lowercase().replace("::", "_") + "_create"`.
  /// For example, the slot `"around_audio_sdk::codec::Codec"` becomes the
  /// symbol `"around_audio_sdk_codec_codec_create"`.
  ///
  /// The framework polls consecutive indices starting from 0 until the
  /// exported function returns a null pointer, signalling the end of
  /// entries for that slot.
  pub fn load(&self, name: &str) -> Result<Arc<Library>, FrameworkError> {
    // ——— RAII guard: removes the loading claim on any exit path ———
    struct LoadingGuard<'a> {
      loading: &'a parking_lot::Mutex<std::collections::HashSet<Box<str>>>,
      name: String,
    }
    impl Drop for LoadingGuard<'_> {
      fn drop(&mut self) {
        self.loading.lock().remove(self.name.as_str());
      }
    }

    // Fast path: already loaded.
    {
      let loaded = self.loaded.read();
      if let Some(lib) = loaded.get(name) {
        return Ok(Arc::clone(lib));
      }
    }

    // Check and claim loading slot — prevents circular dependencies and TOCTOU races.
    {
      let mut loading = self.loading.lock();
      if loading.contains(name) {
        return Err(FrameworkError::Load(format!(
          "circular dependency detected while loading {}",
          name
        )));
      }
      loading.insert(Box::from(name));
    }

    // RAII guard claims the loading slot — on Drop (normal or panic) it
    // removes `name` from the loading set.
    let _guard = LoadingGuard {
      loading: &self.loading,
      name: name.to_string(),
    };

    // Resolve in index — get IndexedMeta with owned strings.
    let (path, indexed) = {
      let index = self.index.read();
      let (path, meta) = index
        .get(name)
        .ok_or_else(|| FrameworkError::NotFound(name.to_string()))?;
      (path.clone(), meta.clone())
    };

    // Validate API version before proceeding further.
    if indexed.api_version != CURRENT_API_VERSION {
      return Err(FrameworkError::ApiVersionMismatch {
        name: name.to_string(),
        expected: CURRENT_API_VERSION,
        got: indexed.api_version,
      });
    }

    // Validate that all required slots are registered before loading dependencies.
    for slot_dep in &indexed.depends_on_slots {
      if !self.slot_map.read().contains_key(slot_dep) {
        return Err(FrameworkError::SlotNotRegistered {
          slot: slot_dep.to_string(),
        });
      }
    }

    // Recursively load dependencies from IndexedMeta.
    for dep in indexed.depends_on() {
      self.load(dep)?;
    }

    // dlopen with RTLD_NOW | RTLD_LOCAL on Unix.
    let lib = unsafe { open_library(&path) }
      .map_err(|e| FrameworkError::Load(format!("dlopen {:?}: {}", path, e)))?;
    let lib = Arc::new(lib);

    // Build heap-allocated ExtensionMeta with CString data for stable FFI pointers.
    let meta_box = indexed.to_heap_ffi();

    // Call around_init if it exists.
    if let Ok(init) = unsafe { lib.get::<unsafe extern "C" fn() -> i32>(b"around_init\0") } {
      let result: Result<i32, FrameworkError> =
        crash_guard_unsafe("around_init", Some(name.to_string()), || unsafe { init() })
          .map_err(Into::into);
      match result {
        Ok(0) => { /* success */ }
        Ok(rc) => {
          return Err(FrameworkError::Init(format!(
            "around_init returned {} for {}",
            rc, name
          )));
        }
        Err(e) => {
          return Err(e);
        }
      }
    }

    // Snapshot slot_map entries before iterating to avoid deadlock
    // if a create() function calls back into attach_register() or unload().
    let slot_entries: Vec<(&'static RegisterVTable, OpaquePtr, Box<str>)> = {
      let slot_map = self.slot_map.read();
      slot_map
        .iter()
        .map(|(name, (vt, reg))| (*vt, *reg, name.clone()))
        .collect()
    };
    for (vt, opaque_reg, slot_name) in &slot_entries {
      // Derive create symbol: lowercased NAME with :: → _, then append _create.
      // E.g. "around_audio_sdk::codec::Codec" → "around_audio_sdk_codec_codec_create"
      let create_sym_name = {
        let base = slot_name.to_lowercase().replace("::", "_");
        format!("{base}_create\0")
      };
      let mut index: usize = 0;
      loop {
        type CreateFn = unsafe extern "C" fn(usize) -> *mut ();
        let create: Symbol<CreateFn> = match unsafe { lib.get(create_sym_name.as_bytes()) } {
          Ok(s) => s,
          Err(_) => break,
        };

        let entry: *mut () = unsafe { create(index) };
        if entry.is_null() {
          break;
        }

        tracing::debug!(slot = slot_name, index, "pushing entry via push_raw");
        let push_result = crash_guard_unsafe("push_raw", Some(name.to_string()), || unsafe {
          (vt.push_raw)(opaque_reg.0, entry, &meta_box.meta as *const ExtensionMeta)
        });
        // Record per-Register trust outcome.
        self.record_guarded_outcome(slot_name, push_result.is_ok());
        push_result.map_err(FrameworkError::from)?;
        index += 1;
      }
    }

    // Save meta pointer before move into loaded_meta.
    let meta_ptr = &meta_box.meta as *const ExtensionMeta;

    // Store the library and heap ExtensionMeta.
    {
      let mut loaded = self.loaded.write();
      loaded.insert(Box::from(name), Arc::clone(&lib));
    }
    {
      let mut loaded_meta = self.loaded_meta.write();
      loaded_meta.insert(Box::from(name), meta_box);
    }

    // Fire OnLoad lifecycle hooks — extension is fully visible now.
    {
      let on_load = self.on_load.lock();
      on_load.broadcast(lifecycle::LifecycleEvent::OnLoad, meta_ptr);
    }

    // Initialize trust counter for this extension (default 100 successes).
    {
      let mut loaded_trust = self.loaded_trust.write();
      loaded_trust.insert(Box::from(name), TrustCounter::new(100));
    }

    // _guard dropped here → removes loading claim.
    tracing::info!(name, "extension loaded");
    Ok(lib)
  }
  /// Unload an extension by name.
  ///
  /// Calls `around_deinit` if the symbol exists, then drops the `Arc<Library>`
  /// which triggers `dlclose` when the last reference is gone.
  pub fn unload(&self, name: &str) -> Result<(), FrameworkError> {
    // Check that no loaded extension depends on this one.
    {
      let loaded_meta = self.loaded_meta.read();
      for (dep_name, dep_meta) in loaded_meta.iter() {
        if **dep_name != *name && dep_meta.meta.depends_on().contains(&name) {
          return Err(FrameworkError::DependentsRemain {
            name: name.to_string(),
            dependent: dep_name.to_string(),
          });
        }
      }
    }
    // Remove from loaded_meta first (we need the meta pointer for remove_by_meta).
    let meta_box = {
      let mut loaded_meta = self.loaded_meta.write();
      loaded_meta
        .remove(name)
        .ok_or_else(|| FrameworkError::NotFound(name.to_string()))?
    };

    // Call remove_by_meta on all slot registers for this extension.
    {
      let slot_map = self.slot_map.read();
      for (_slot_name, (vt, opaque_reg)) in slot_map.iter() {
        tracing::debug!(name, "calling remove_by_meta for extension");
        // SAFETY: reg is a valid Register pointer; meta_box.meta is valid.
        unsafe { (vt.remove_by_meta)(opaque_reg.0, &meta_box.meta as *const ExtensionMeta) };
      }
    }

    // Remove the loaded library.
    let lib = {
      let mut loaded = self.loaded.write();
      loaded
        .remove(name)
        .ok_or_else(|| FrameworkError::NotFound(name.to_string()))?
    };
    // Remove trust counter.
    {
      let mut loaded_trust = self.loaded_trust.write();
      loaded_trust.remove(name);
    }

    // Fire OnUnload lifecycle hooks before around_deinit.
    {
      let on_unload = self.on_unload.lock();
      on_unload.broadcast(
        lifecycle::LifecycleEvent::OnUnload,
        &meta_box.meta as *const ExtensionMeta,
      );
    }

    // Call around_deinit if it exists.
    // SAFETY: deinit is `unsafe extern "C" fn()`.
    if let Ok(deinit) = unsafe { lib.get::<unsafe extern "C" fn()>(b"around_deinit\0") } {
      tracing::debug!(name, "calling around_deinit");
      if let Err(crash) = crash_guard_unsafe("around_deinit", Some(name.to_string()), || unsafe {
        deinit()
      }) {
        tracing::error!(
          name,
          ?crash,
          "around_deinit panicked; continuing with unload"
        );
        // Do NOT abort unload — the extension is being removed anyway.
      }
    }
    tracing::info!(name, "extension unloaded");
    // `lib` is dropped here → dlclose when last Arc reference gone.
    // `meta_box` is dropped here → frees all OwnedFFIMeta allocations.
    Ok(())
  }

  /// Return whether an extension is currently loaded.
  pub fn is_loaded(&self, name: &str) -> bool {
    self.loaded.read().contains_key(name)
  }

  /// Return the number of scanned extensions in the index.
  pub fn index_len(&self) -> usize {
    self.index.read().len()
  }

  /// Return the number of loaded extensions.
  pub fn loaded_len(&self) -> usize {
    self.loaded.read().len()
  }

  /// Return a snapshot of the scanned extension index.
  /// Returns `(name, path)` pairs for all scanned extensions.
  pub fn index_entries(&self) -> Vec<(String, PathBuf)> {
    self
      .index
      .read()
      .iter()
      .map(|(name, (path, _))| (name.to_string(), path.clone()))
      .collect()
  }

  /// Phase 1 of the two-phase hot-reload protocol.
  ///
  /// Loads a new extension `.so` from `path`, reads its metadata, polls all
  /// registered slots for entries, and returns a [`PendingReload`] handle
  /// while the old extension continues serving requests.
  ///
  /// The returned `PendingReload` is consumed by
  /// [`commit_reload`](Self::commit_reload).
  pub fn prepare_reload(&self, path: &Path) -> Result<PendingReload, FrameworkError> {
    // dlopen the new .so with RTLD_NOW | RTLD_LOCAL.
    let lib = unsafe { open_library(path) }
      .map_err(|e| FrameworkError::Load(format!("prepare_reload dlopen {:?}: {}", path, e)))?;
    let lib = Arc::new(lib);

    // Read AROUND_META and build OwnedFFIMeta with owned backing strings.
    let meta_sym: Symbol<&'static ExtensionMeta> = unsafe {
      lib
        .get(b"AROUND_META\0")
        .map_err(|_| FrameworkError::Load(format!("no AROUND_META in {:?}", path)))?
    };

    // Read AROUND_SLOTS (V0: slot definitions from the extension are
    // deferred — extensions typically provide entries to host-registered
    // slots rather than defining new slot types).
    let slot_defs: Vec<SlotDef> = Vec::new();
    let meta_box = IndexedMeta::from_ffi(*meta_sym).to_heap_ffi();
    // Snapshot slot_map entries before iterating to avoid deadlock
    // if a create() function calls back into attach_register() (B5).
    let slot_entries: Vec<(Box<str>, &'static RegisterVTable, OpaquePtr)> = {
      let slot_map = self.slot_map.read();
      slot_map
        .iter()
        .map(|(name, (vt, reg))| (name.clone(), *vt, *reg))
        .collect()
    };
    // Lock released — safe to call create().
    let entries: HashMap<Box<str>, Vec<*mut ()>> = {
      let mut all_entries = HashMap::new();
      for (slot_name, _vt, _opaque_reg) in &slot_entries {
        let create_sym_name = {
          let base = slot_name.to_lowercase().replace("::", "_");
          format!("{base}_create\0")
        };
        let mut slot_entries: Vec<*mut ()> = Vec::new();
        let mut index: usize = 0;
        loop {
          type CreateFn = unsafe extern "C" fn(usize) -> *mut ();
          let create: Symbol<CreateFn> = match unsafe { lib.get(create_sym_name.as_bytes()) } {
            Ok(s) => s,
            Err(_) => break,
          };
          let entry: *mut () = unsafe { create(index) };
          if entry.is_null() {
            break;
          }
          slot_entries.push(entry);
          index += 1;
        }
        if !slot_entries.is_empty() {
          all_entries.insert(slot_name.clone(), slot_entries);
        }
      }
      all_entries
    };

    // Call around_init to validate it succeeds before commit phase (B4).
    if let Ok(init) = unsafe { lib.get::<unsafe extern "C" fn() -> i32>(b"around_init\0") } {
      let ext_name = meta_box.meta.name();
      let result: Result<i32, FrameworkError> =
        crash_guard_unsafe("around_init", Some(ext_name.to_string()), || unsafe {
          init()
        })
        .map_err(Into::into);
      match result {
        Ok(0) => { /* success */ }
        Ok(rc) => {
          return Err(FrameworkError::Init(format!(
            "around_init returned {} for {:?}",
            rc, path
          )));
        }
        Err(e) => {
          return Err(e);
        }
      }
    }

    // Convert depends_on from the new meta into owned strings.
    let depends_on: Vec<Box<str>> = meta_box
      .meta
      .depends_on()
      .into_iter()
      .map(Box::from)
      .collect();

    Ok(PendingReload {
      lib,
      meta: meta_box,
      slot_defs,
      entries,
      depends_on,
    })
  }

  /// Phase 2 of the two-phase hot-reload protocol.
  ///
  /// Atomically replaces the extension identified by `old_meta` with the new
  /// extension state in `pending`. Fires [`LifecycleEvent::OnReload`] before
  /// the swap, then [`LifecycleEvent::OnUnload`], removes old entries, calls
  /// `around_deinit`, drops the old library, calls `around_init` on the new
  /// extension, pushes new entries, and broadcasts [`LifecycleEvent::OnLoad`].
  ///
  /// # Safety
  ///
  /// `old_meta` must be a valid pointer to an `ExtensionMeta` of a currently
  /// loaded extension. The pointer must remain valid for the duration of this
  /// call. In practice, obtain `old_meta` via `Framework::loaded_meta(name)`
  /// or from the [`reload`](Self::reload) convenience method.
  pub fn commit_reload(
    &self,
    old_meta: *const ExtensionMeta,
    pending: PendingReload,
  ) -> Result<(), FrameworkError> {
    // ── Find old extension name by meta pointer. ──
    let old_name: Box<str> = {
      let loaded_meta = self.loaded_meta.read();
      let mut found: Option<Box<str>> = None;
      for (name, boxed_meta) in loaded_meta.iter() {
        if &boxed_meta.meta as *const ExtensionMeta == old_meta {
          found = Some(name.clone());
          break;
        }
      }
      found.ok_or_else(|| {
        FrameworkError::NotFound("meta pointer not found in loaded extensions".to_string())
      })?
    };

    // Verify no other loaded extension depends on the old one.
    {
      let loaded_meta = self.loaded_meta.read();
      for (dep_name, dep_meta) in loaded_meta.iter() {
        if **dep_name != *old_name && dep_meta.meta.depends_on().contains(&old_name.as_ref()) {
          return Err(FrameworkError::DependentsRemain {
            name: old_name.to_string(),
            dependent: dep_name.to_string(),
          });
        }
      }
    }

    // ── Phase 1: Lifecycle broadcasts (no write locks held). ──
    // Broadcast BEFORE removal so handlers can still query old state (B3).

    // Broadcast OnReload — old extension still visible in loaded/loaded_meta.
    {
      let on_reload = self.on_reload.lock();
      on_reload.broadcast(lifecycle::LifecycleEvent::OnReload, old_meta);
    }

    // Broadcast OnUnload — old extension still visible.
    {
      let on_unload = self.on_unload.lock();
      on_unload.broadcast(lifecycle::LifecycleEvent::OnUnload, old_meta);
    }

    // ── Phase 2: Remove old extension (write locks acquired, then released). ──
    let old_meta_box: Box<OwnedFFIMeta>;
    let old_lib: Arc<Library>;
    {
      let mut loaded = self.loaded.write();
      let mut loaded_meta = self.loaded_meta.write();
      old_meta_box = loaded_meta.remove(&old_name).ok_or_else(|| {
        FrameworkError::NotFound(format!("{} not found in loaded_meta", old_name))
      })?;
      old_lib = loaded
        .remove(&old_name)
        .ok_or_else(|| FrameworkError::NotFound(format!("{} not found in loaded", old_name)))?;
    }
    // Write locks dropped here — safe for lifecycle hooks to query state.

    // Remove entries from all slot registers.
    {
      let slot_map = self.slot_map.read();
      for (_slot_name, (vt, opaque_reg)) in slot_map.iter() {
        tracing::debug!(name = &*old_name, "commit_reload: remove_by_meta");
        // SAFETY: reg is a valid Register pointer; old_meta_box.meta is valid.
        unsafe { (vt.remove_by_meta)(opaque_reg.0, &old_meta_box.meta as *const ExtensionMeta) };
      }
    }

    // Call around_deinit on the old extension.
    if let Ok(deinit) = unsafe { old_lib.get::<unsafe extern "C" fn()>(b"around_deinit\0") } {
      tracing::debug!(
        name = &*old_name,
        "commit_reload: calling around_deinit (old)"
      );
      if let Err(crash) =
        crash_guard_unsafe("around_deinit", Some(old_name.to_string()), || unsafe {
          deinit()
        })
      {
        tracing::error!(
          name = &*old_name,
          ?crash,
          "around_deinit panicked; continuing reload"
        );
      }
    }

    // ── Point of no return: old extension is fully gone. ──
    drop(old_lib);
    drop(old_meta_box);

    // around_init was already called in prepare_reload (B4) — skip here.

    // ── Phase 3: Install new extension. ──
    let new_meta_ptr = &pending.meta.meta as *const ExtensionMeta;

    // Push entries for the new extension into each slot register.
    for (slot_name, entries) in &pending.entries {
      let slot_map = self.slot_map.read();
      if let Some((vt, opaque_reg)) = slot_map.get(slot_name) {
        for &entry in entries {
          crash_guard_unsafe("push_raw", Some(old_name.to_string()), || unsafe {
            (vt.push_raw)(opaque_reg.0, entry, new_meta_ptr)
          })
          .map_err(FrameworkError::from)?;
        }
      } else {
        tracing::warn!(
          slot = &**slot_name,
          "commit_reload: slot not registered, skipping entries"
        );
      }
    }

    // Store new library and meta.
    {
      let mut loaded = self.loaded.write();
      let mut loaded_meta = self.loaded_meta.write();
      loaded.insert(old_name.clone(), Arc::clone(&pending.lib));
      loaded_meta.insert(old_name.clone(), pending.meta);
    }

    // Reset trust counter for the replaced extension (starts fresh).
    {
      let mut loaded_trust = self.loaded_trust.write();
      loaded_trust.insert(old_name.clone(), TrustCounter::new(100));
    }

    // Broadcast OnLoad lifecycle event.
    {
      let on_load = self.on_load.lock();
      on_load.broadcast(lifecycle::LifecycleEvent::OnLoad, new_meta_ptr);
    }

    tracing::info!(name = &*old_name, "extension hot-reloaded");
    Ok(())
  }

  /// Convenience method: prepare + commit in one call.
  ///
  /// Equivalent to [`prepare_reload`](Self::prepare_reload) + [`commit_reload`](Self::commit_reload).
  pub fn reload(&self, old_name: &str, new_path: &Path) -> Result<(), FrameworkError> {
    // Resolve the old extension's meta pointer while holding a read lock.
    let old_meta: *const ExtensionMeta = {
      let loaded_meta = self.loaded_meta.read();
      let boxed = loaded_meta
        .get(old_name)
        .ok_or_else(|| FrameworkError::NotFound(old_name.to_string()))?;
      &boxed.meta as *const ExtensionMeta
    };

    let pending = self.prepare_reload(new_path)?;
    self.commit_reload(old_meta, pending)
  }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`Framework`] operations.
#[derive(Debug, Clone)]
pub enum FrameworkError {
  Io(String),
  NotFound(String),
  Load(String),
  Init(String),
  ApiVersionMismatch {
    name: String,
    expected: u32,
    got: u32,
  },
  DependentsRemain {
    name: String,
    dependent: String,
  },
  SlotNotRegistered {
    slot: String,
  },
  /// An extension operation crashed (panicked).
  Crashed {
    /// Which guarded operation panicked.
    operation: &'static str,
    /// The extension name, if known.
    extension: Option<String>,
    /// The panic payload.
    payload: Option<String>,
  },
}

impl fmt::Display for FrameworkError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      FrameworkError::Io(msg) => write!(f, "I/O error: {}", msg),
      FrameworkError::NotFound(name) => write!(f, "extension not found: {}", name),
      FrameworkError::Load(msg) => write!(f, "load error: {}", msg),
      FrameworkError::Init(msg) => write!(f, "init error: {}", msg),
      FrameworkError::ApiVersionMismatch {
        name,
        expected,
        got,
      } => {
        write!(
          f,
          "API version mismatch for {}: expected {}, got {}",
          name, expected, got
        )
      }
      FrameworkError::DependentsRemain { name, dependent } => {
        write!(
          f,
          "cannot unload {}: {} still depends on it",
          name, dependent
        )
      }
      FrameworkError::SlotNotRegistered { slot } => {
        write!(f, "required slot not registered: {}", slot)
      }
      FrameworkError::Crashed {
        operation,
        extension,
        payload,
      } => {
        let ext = extension.as_deref().unwrap_or("<unknown>");
        let msg = payload.as_deref().unwrap_or("<no payload>");
        write!(f, "{} crashed in '{}': {}", operation, ext, msg)
      }
    }
  }
}

impl std::error::Error for FrameworkError {}

impl From<CrashError> for FrameworkError {
  fn from(err: CrashError) -> Self {
    FrameworkError::Crashed {
      operation: err.operation,
      extension: err.extension,
      payload: err.payload,
    }
  }
}
