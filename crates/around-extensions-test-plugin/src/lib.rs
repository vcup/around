//! Minimal test extension for the around-extensions framework.
//!
//! Exports `AROUND_META` and `TestSlot_create` so integration tests can
//! verify scan, load, unload, and remove_by_meta lifecycle.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Extension metadata
// ---------------------------------------------------------------------------

#[no_mangle]
pub static AROUND_META: around_extensions::ExtensionMeta = around_extensions::ExtensionMeta {
  name: c"around-extensions-test-plugin".as_ptr().cast(),
  semver: c"0.1.0".as_ptr().cast(),
  api_version: 1,
  _pad: 0,
  depends_on: std::ptr::null(),
  depends_on_slots: std::ptr::null(),
  author: c"around project".as_ptr().cast(),
  description: c"Test plugin for around-extensions framework tests"
    .as_ptr()
    .cast(),
};

// ---------------------------------------------------------------------------
// Init / Deinit tracking
// ---------------------------------------------------------------------------

static INIT_CALLED: AtomicBool = AtomicBool::new(false);
static DEINIT_CALLED: AtomicBool = AtomicBool::new(false);

/// Returns whether `around_init` was called (for test assertions).
#[no_mangle]
pub extern "C" fn test_plugin_init_called() -> i32 {
  if INIT_CALLED.load(Ordering::SeqCst) {
    1
  } else {
    0
  }
}

/// Returns whether `around_deinit` was called (for test assertions).
#[no_mangle]
pub extern "C" fn test_plugin_deinit_called() -> i32 {
  if DEINIT_CALLED.load(Ordering::SeqCst) {
    1
  } else {
    0
  }
}

/// Reset tracking flags (called between tests).
#[no_mangle]
pub extern "C" fn test_plugin_reset_flags() {
  INIT_CALLED.store(false, Ordering::SeqCst);
  DEINIT_CALLED.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Extension lifecycle
// ---------------------------------------------------------------------------

/// # Safety
///
/// The framework calls this function exactly once after `dlopen`, before any
/// create functions. It must not be called concurrently with any other
/// extension function.
#[no_mangle]
pub unsafe extern "C" fn around_init() -> i32 {
  INIT_CALLED.store(true, Ordering::SeqCst);
  0 // success
}

/// # Safety
///
/// The framework calls this function exactly once before `dlclose`. It must not
/// be called concurrently with any other extension function.
#[no_mangle]
pub unsafe extern "C" fn around_deinit() {
  DEINIT_CALLED.store(true, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Slot: TestSlot — simple test entries
// ---------------------------------------------------------------------------

/// Test entry returned by create: holds an id.
#[repr(C)]
pub struct TestEntry {
  pub id: u32,
}

/// Factory for dynamic loading. Returns one entry for index 0, null for >=1.
///
/// # Safety
///
/// `index` must be the slot index assigned by the framework (caller
/// guarantees sequential calls with `index` starting from 0).
#[no_mangle]
pub unsafe extern "C" fn testslot_create(index: usize) -> *mut std::ffi::c_void {
  if index == 0 {
    let entry = Box::new(TestEntry { id: 42 });
    Box::into_raw(entry) as *mut std::ffi::c_void
  } else {
    std::ptr::null_mut()
  }
}

// ---------------------------------------------------------------------------
// Extension-owned slot — exercises AROUND_SLOTS lifecycle
// ---------------------------------------------------------------------------

#[repr(C)]
struct OwnedSlotDef {
  slot_name: &'static str,
  reg_vtable: *const (),
  reg_instance: *mut (),
}

// SAFETY: every field points to immutable static storage.
unsafe impl Sync for OwnedSlotDef {}

static OWNED_PUSH_COUNT: AtomicUsize = AtomicUsize::new(0);
static OWNED_REMOVE_COUNT: AtomicUsize = AtomicUsize::new(0);
static OWNED_REGISTER: u8 = 0;

unsafe extern "C" fn owned_push_raw(
  _reg: *mut (),
  entry: *mut (),
  _meta: *const around_extensions::ExtensionMeta,
) {
  OWNED_PUSH_COUNT.fetch_add(1, Ordering::SeqCst);
  if !entry.is_null() {
    // SAFETY: ownedtestslot_create allocates every non-null entry as TestEntry.
    unsafe {
      drop(Box::from_raw(entry.cast::<TestEntry>()));
    }
  }
}

unsafe extern "C" fn owned_remove_by_meta(
  _reg: *mut (),
  _meta: *const around_extensions::ExtensionMeta,
) -> usize {
  OWNED_REMOVE_COUNT.fetch_add(1, Ordering::SeqCst);
  0
}

static OWNED_VTABLE: around_extensions::RegisterVTable = around_extensions::RegisterVTable {
  push_raw: owned_push_raw,
  remove_by_meta: owned_remove_by_meta,
};

#[no_mangle]
static AROUND_SLOTS: &[OwnedSlotDef] = &[OwnedSlotDef {
  slot_name: "OwnedTestSlot",
  reg_vtable: std::ptr::addr_of!(OWNED_VTABLE).cast(),
  reg_instance: std::ptr::addr_of!(OWNED_REGISTER).cast_mut().cast(),
}];

/// Create the sole entry for the extension-owned test slot.
///
/// # Safety
///
/// The framework must poll sequential indices beginning at zero and take
/// ownership of every non-null pointer returned.
#[no_mangle]
pub unsafe extern "C" fn ownedtestslot_create(index: usize) -> *mut std::ffi::c_void {
  if index == 0 {
    Box::into_raw(Box::new(TestEntry { id: 84 })).cast()
  } else {
    std::ptr::null_mut()
  }
}

#[no_mangle]
pub extern "C" fn owned_slot_push_count() -> usize {
  OWNED_PUSH_COUNT.load(Ordering::SeqCst)
}

#[no_mangle]
pub extern "C" fn owned_slot_remove_count() -> usize {
  OWNED_REMOVE_COUNT.load(Ordering::SeqCst)
}

#[no_mangle]
pub extern "C" fn owned_slot_reset_counts() {
  OWNED_PUSH_COUNT.store(0, Ordering::SeqCst);
  OWNED_REMOVE_COUNT.store(0, Ordering::SeqCst);
}
