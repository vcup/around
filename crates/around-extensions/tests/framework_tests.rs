//! Integration tests for the around-extensions framework.
//!
//! Uses the `around-extensions-test-plugin` cdylib crate for real `.so` loading,
//! verifying the full scan → load → unload lifecycle and callback contracts.
//!
//! The framework (`Framework::instance()`) is a process-wide singleton, so all
//! tests are serialized via a global mutex to avoid cross-test interference.

use around_extensions::*;
use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Platform-specific plugin filename parts
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
const PLUGIN_PREFIX: &str = "libaround_extensions_test_plugin";
#[cfg(target_os = "macos")]
const PLUGIN_PREFIX: &str = "libaround_extensions_test_plugin";
#[cfg(target_os = "windows")]
const PLUGIN_PREFIX: &str = "around_extensions_test_plugin";
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const PLUGIN_PREFIX: &str = "libaround_extensions_test_plugin";

#[cfg(target_os = "linux")]
const PLUGIN_EXT: &str = ".so";
#[cfg(target_os = "macos")]
const PLUGIN_EXT: &str = ".dylib";
#[cfg(target_os = "windows")]
const PLUGIN_EXT: &str = ".dll";
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const PLUGIN_EXT: &str = ".so";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate the compiled test plugin `.so` / `.dylib` / `.dll` in the workspace
/// `target/debug/` directory.
///
/// Uses `CARGO_MANIFEST_DIR` (set by `cargo test`) to find the workspace root,
/// then searches `target/debug/` for files matching the expected pattern.
fn find_test_plugin() -> PathBuf {
  let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
    .expect("CARGO_MANIFEST_DIR must be set — run under cargo test");
  let manifest = PathBuf::from(&manifest_dir);
  let workspace_root = manifest
    .parent()
    .expect("around-extensions lives in crates/")
    .parent()
    .expect("crates/ lives at workspace root");
  let target_dir = workspace_root.join("target").join("debug");

  let mut candidates: Vec<PathBuf> = std::fs::read_dir(&target_dir)
    .unwrap_or_else(|e| panic!("cannot read directory {:?}: {}", target_dir, e))
    .filter_map(|e| e.ok())
    .map(|e| e.path())
    .filter(|p| {
      p.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
          // Cargo may add a hash suffix (e.g. `-abcdef`) between
          // the base name and the extension — use `contains`.
          n.starts_with(PLUGIN_PREFIX) && n.contains(PLUGIN_EXT) && !n.ends_with(".d")
        })
        .unwrap_or(false)
    })
    .collect();

  // Sort so the plain (non-hashed) name wins when present.
  candidates.sort();
  candidates.into_iter().next().unwrap_or_else(|| {
    panic!(
      "test plugin not found in {:?}\n\
             (looking for prefix={:?}, ext={:?})\n\
             Build it with: cargo build -p around-extensions-test-plugin",
      target_dir, PLUGIN_PREFIX, PLUGIN_EXT,
    )
  })
}

/// Acquire the global framework-test lock and initialise tracing.
///
/// Because `Framework::instance()` is a process-wide singleton, framework
/// tests must not run concurrently.  This function returns a guard whose
/// `Drop` releases the serialisation lock.
///
/// Tracing initialisation is attempted once — subsequent calls are silently
/// ignored by `try_init`.
fn test_setup() -> impl Drop {
  let _ = tracing_subscriber::fmt().with_test_writer().try_init();

  static MTX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
  MTX.lock()
}

// ---------------------------------------------------------------------------
// Test-slot callback tracking
// ---------------------------------------------------------------------------
//
// Minimal `RegisterVTable` implementation that counts invocations.
// Used by tests that verify `push_raw` / `remove_by_meta` are called.

struct TestSlotRecords {
  push_count: usize,
  remove_count: usize,
}

static TEST_RECORDS: parking_lot::Mutex<TestSlotRecords> =
  parking_lot::Mutex::new(TestSlotRecords {
    push_count: 0,
    remove_count: 0,
  });

unsafe extern "C" fn test_push_raw(_reg: *mut (), _entry: *mut (), _meta: *const ExtensionMeta) {
  TEST_RECORDS.lock().push_count += 1;
}

unsafe extern "C" fn test_remove_by_meta(_reg: *mut (), _meta: *const ExtensionMeta) -> usize {
  TEST_RECORDS.lock().remove_count += 1;
  1
}

static TEST_VTABLE: RegisterVTable = RegisterVTable {
  push_raw: test_push_raw,
  remove_by_meta: test_remove_by_meta,
};

/// Reset the callback counters to zero.
fn reset_test_records() {
  let mut r = TEST_RECORDS.lock();
  r.push_count = 0;
  r.remove_count = 0;
}

/// Plugin name constant used by all load/unload calls.
const PLUGIN_NAME: &str = "around-extensions-test-plugin";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn scan_discovers_extension() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  fw.scan(&[plugin_path.parent().unwrap().to_path_buf()])
    .unwrap();

  assert!(
    fw.index_len() >= 1,
    "scan should discover at least one extension in the target directory"
  );
}

#[test]
fn scan_extension_meta_correct() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  fw.scan(&[plugin_path.parent().unwrap().to_path_buf()])
    .unwrap();

  let lib = fw.load(PLUGIN_NAME).unwrap();

  // Read `AROUND_META` from the loaded library and verify its fields.
  unsafe {
    let meta: &ExtensionMeta = *lib
      .get::<&ExtensionMeta>(b"AROUND_META\0")
      .expect("AROUND_META symbol must exist");

    assert_eq!(meta.name(), PLUGIN_NAME);
    assert_eq!(meta.semver(), "0.1.0");
    assert_eq!(meta.api_version, 1);
  }

  // Cleanup so subsequent tests start with a clean state.
  fw.unload(PLUGIN_NAME).unwrap();
}

#[test]
fn load_calls_around_init() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  fw.scan(&[plugin_path.parent().unwrap().to_path_buf()])
    .unwrap();

  let lib = fw.load(PLUGIN_NAME).unwrap();

  // After load the framework has called `around_init` — verify the flag.
  unsafe {
    let init_called: libloading::Symbol<extern "C" fn() -> i32> = lib
      .get(b"test_plugin_init_called\0")
      .expect("test_plugin_init_called symbol must exist");
    assert_eq!(
      init_called(),
      1,
      "around_init should have been called before load() returns"
    );
  }

  // Unload — internally calls `around_deinit`.
  fw.unload(PLUGIN_NAME).unwrap();

  // We still hold our Arc<Library>, so the library is still mapped and
  // we can verify the deinit flag.
  unsafe {
    let deinit_called: libloading::Symbol<extern "C" fn() -> i32> = lib
      .get(b"test_plugin_deinit_called\0")
      .expect("test_plugin_deinit_called symbol must exist");
    assert_eq!(
      deinit_called(),
      1,
      "around_deinit should have been called during unload()"
    );
  }

  // Drop our reference so the library is fully closed.
  drop(lib);
}

#[test]
fn unload_calls_remove_by_meta() {
  let _guard = test_setup();
  reset_test_records();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  // Register a test slot before loading.
  fw.attach_register("TestSlot", &TEST_VTABLE, std::ptr::null_mut());

  fw.scan(&[dir]).unwrap();
  let lib = fw.load(PLUGIN_NAME).unwrap();

  // The framework polls `TestSlot_create(index)` for each registered slot.
  // `TestSlot_create(0)` returns a `TestEntry { id: 42 }`, so push_raw
  // should have been called once.
  {
    let r = TEST_RECORDS.lock();
    assert!(
      r.push_count >= 1,
      "push_raw should have been called during load (push_count = {})",
      r.push_count
    );
  }

  // Unload — for every registered slot, `remove_by_meta` is called.
  fw.unload(PLUGIN_NAME).unwrap();

  {
    let r = TEST_RECORDS.lock();
    assert!(
      r.remove_count >= 1,
      "remove_by_meta should have been called during unload (remove_count = {})",
      r.remove_count
    );
  }

  drop(lib);
}

#[test]
fn duplicate_scan_ignored() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  fw.scan(&[dir.clone()]).unwrap();
  let after_first = fw.index_len();

  fw.scan(&[dir]).unwrap();
  let after_second = fw.index_len();

  assert_eq!(
    after_first, after_second,
    "duplicate scan should not increase index_len ({} vs {})",
    after_first, after_second
  );
}

#[test]
fn load_nonexistent_errors() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let result = fw.load("nonexistent");

  assert!(
    matches!(result, Err(FrameworkError::NotFound(_))),
    "loading a nonexistent extension should return NotFound, got {:?}",
    result
  );
}

#[test]
fn attach_register_and_load() {
  let _guard = test_setup();
  reset_test_records();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  // Register the test slot BEFORE loading.
  fw.attach_register("TestSlot", &TEST_VTABLE, std::ptr::null_mut());

  fw.scan(&[dir]).unwrap();
  let lib = fw.load(PLUGIN_NAME).unwrap();

  // The framework must poll `TestSlot_create` for each registered slot.
  // Because the slot is registered before load, the framework will:
  //   TestSlot_create(0) → non-null → push_raw called
  //   TestSlot_create(1) → null → stop
  {
    let r = TEST_RECORDS.lock();
    assert!(
      r.push_count >= 1,
      "framework should have polled TestSlot_create and called push_raw (count = {})",
      r.push_count
    );
  }

  // Verify that unload still triggers remove_by_meta.
  reset_test_records();
  fw.unload(PLUGIN_NAME).unwrap();
  {
    let r = TEST_RECORDS.lock();
    assert!(
      r.remove_count >= 1,
      "remove_by_meta should be called during unload (count = {})",
      r.remove_count
    );
  }

  drop(lib);
}

#[test]
fn is_loaded_after_load() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  fw.scan(&[dir]).unwrap();
  assert!(!fw.is_loaded(PLUGIN_NAME));

  let lib = fw.load(PLUGIN_NAME).unwrap();
  assert!(fw.is_loaded(PLUGIN_NAME));

  fw.unload(PLUGIN_NAME).unwrap();
  assert!(!fw.is_loaded(PLUGIN_NAME));

  drop(lib);
}

#[test]
fn loaded_len_tracks_active_extensions() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  fw.scan(&[dir]).unwrap();
  assert_eq!(fw.loaded_len(), 0, "nothing loaded yet");

  let lib = fw.load(PLUGIN_NAME).unwrap();
  assert_eq!(fw.loaded_len(), 1, "one extension should be loaded");

  fw.unload(PLUGIN_NAME).unwrap();
  assert_eq!(fw.loaded_len(), 0, "no extensions after unload");

  drop(lib);
}

// ---------------------------------------------------------------------------
// Critical coverage: idempotent load, load-after-unload, scan errors
// ---------------------------------------------------------------------------

#[test]
fn load_is_idempotent() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  fw.scan(&[dir]).unwrap();

  let lib1 = fw.load(PLUGIN_NAME).unwrap();
  assert_eq!(fw.loaded_len(), 1);

  // Second load must return immediately without re-init or re-push.
  let lib2 = fw.load(PLUGIN_NAME).unwrap();
  assert_eq!(
    fw.loaded_len(),
    1,
    "loaded_len must not increase on idempotent load"
  );
  assert!(
    Arc::ptr_eq(&lib1, &lib2),
    "idempotent load must return the same Arc<Library>"
  );

  fw.unload(PLUGIN_NAME).unwrap();
  drop(lib1);
  drop(lib2);
}

#[test]
fn load_after_unload_round_trip() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let plugin_path = find_test_plugin();
  let dir = plugin_path.parent().unwrap().to_path_buf();

  fw.scan(&[dir]).unwrap();

  // First load.
  let lib = fw.load(PLUGIN_NAME).unwrap();
  assert!(fw.is_loaded(PLUGIN_NAME));
  assert_eq!(fw.loaded_len(), 1);
  drop(lib);

  // Unload.
  fw.unload(PLUGIN_NAME).unwrap();
  assert!(!fw.is_loaded(PLUGIN_NAME));
  assert_eq!(fw.loaded_len(), 0);

  // Re-load — must be a fresh load, not a stale cache hit.
  let lib2 = fw.load(PLUGIN_NAME).unwrap();
  assert!(fw.is_loaded(PLUGIN_NAME));
  assert_eq!(fw.loaded_len(), 1);

  fw.unload(PLUGIN_NAME).unwrap();
  drop(lib2);
}

#[test]
fn scan_nonexistent_directory_errors() {
  let _guard = test_setup();
  let fw = Framework::instance();

  let bogus = std::path::PathBuf::from("/tmp/around_nonexistent_dir_xyz123");
  // Ensure the path really doesn't exist.
  assert!(
    !bogus.exists(),
    "test precondition: bogus dir must not exist"
  );

  let result = fw.scan(&[bogus]);
  assert!(
    matches!(result, Err(FrameworkError::Io(_))),
    "scan of nonexistent directory must return Io error, got {:?}",
    result
  );
}
