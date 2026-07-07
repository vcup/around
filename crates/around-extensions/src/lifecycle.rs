//! Lifecycle hooks for the extension framework.
//!
//! Provides [`LifecycleRegister`] — a notification dispatcher that broadcasts
//! lifecycle events ([`LifecycleEvent`]) to registered callbacks.
//!
//! Callbacks are stored as `extern "C"` function pointers (no vtable dispatch
//! during delicate framework state). A safe [`register`] wrapper boxes Rust
//! closures into a trampoline that catches panics internally.
use crate::ExtensionMeta;

/// Events that can be broadcast to lifecycle hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleEvent {
  OnLoad,
  OnUnload,
  OnReload,
}

/// Lifecycle hook callback (extern "C" ABI, no vtable dispatch).
///
/// `ext_meta` points to the [`ExtensionMeta`] of the extension being
/// loaded/unloaded/reloaded. The pointer is valid only for the duration
/// of the call — listeners MUST NOT retain it.
/// `context` is an opaque pointer supplied at registration time.
pub(crate) type LifecycleFn = unsafe extern "C" fn(meta: *const ExtensionMeta, context: *mut ());

/// A single lifecycle hook registration.
pub(crate) struct LifecycleEntry {
  /// The callback function (extern "C" ABI).
  pub(crate) callback: LifecycleFn,
  /// Opaque context pointer passed to the callback.
  pub(crate) context: *mut (),
  /// Optional human-readable label for diagnostics.
  pub(crate) name: Option<String>,
  /// If true, `context` is a boxed `Box<dyn Fn>` that must be freed on drop.
  free_context: bool,
}

impl Drop for LifecycleEntry {
  fn drop(&mut self) {
    if self.free_context {
      // Reconstruct the Box<dyn Fn> and let it drop, freeing the allocation.
      unsafe {
        let _ = Box::from_raw(self.context as *mut Box<dyn Fn(*const ExtensionMeta) + Send + Sync>);
      }
    }
  }
}

// SAFETY: LifecycleEntry stores opaque context pointers (`*mut ()`) that are
// only dereferenced during broadcast, which is serialized by the outer Mutex
// around LifecycleRegister. The callback is an extern "C" fn pointer (always
// Sync). This mirrors the safety reasoning of OpaquePtr in the framework.
unsafe impl Send for LifecycleEntry {}
unsafe impl Sync for LifecycleEntry {}

/// A registry of lifecycle hooks that are broadcast on extension lifecycle
/// events (load, unload, reload).
pub(crate) struct LifecycleRegister {
  hooks: Vec<LifecycleEntry>,
}
impl LifecycleRegister {
  pub(crate) fn new() -> Self {
    Self { hooks: Vec::new() }
  }

  /// Register a lifecycle hook via a Rust closure (safe wrapper).
  ///
  /// The closure is boxed into a heap allocation and called through a
  /// non-generic `extern "C"` trampoline, keeping the storage type as a
  /// raw function pointer (no vtable at the broadcast site).
  #[expect(
    dead_code,
    reason = "API for external hook registration; unused until Framework::register_lifecycle_hook is called"
  )]
  pub(crate) fn register(
    &mut self,
    name: Option<String>,
    hook: Box<dyn Fn(*const ExtensionMeta) + Send + Sync>,
  ) {
    // Trampoline: non-generic extern "C" fn that calls the boxed closure.
    // catch_unwind is required because extern "C" frames cannot safely unwind.
    unsafe extern "C" fn closure_trampoline(meta: *const ExtensionMeta, context: *mut ()) {
      let f: &Box<dyn Fn(*const ExtensionMeta) + Send + Sync> =
        unsafe { &*(context as *const Box<dyn Fn(*const ExtensionMeta) + Send + Sync>) };
      if let Err(panic_payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f(meta);
      })) {
        let msg: &str = panic_payload
          .downcast_ref::<&str>()
          .copied()
          .or_else(|| panic_payload.downcast_ref::<String>().map(|s| s.as_str()))
          .unwrap_or("<non-string panic>");
        tracing::error!("lifecycle hook panicked: {}", msg);
      }
    }
    let context = Box::into_raw(Box::new(hook)) as *mut ();
    self.hooks.push(LifecycleEntry {
      callback: closure_trampoline,
      context,
      name,
      free_context: true,
    });
  }

  /// Register a raw `extern "C"` lifecycle hook (for C ABI callers).
  ///
  /// # Safety
  ///
  /// `callback` must be a valid `extern "C"` function pointer. If `context`
  /// is non-null, the caller is responsible for its lifetime — it is never
  /// freed by the framework.
  #[expect(
    dead_code,
    reason = "C ABI callers use this; unused until cabi.rs registers hooks"
  )]
  pub(crate) fn register_raw(
    &mut self,
    name: Option<String>,
    callback: unsafe extern "C" fn(*const ExtensionMeta, *mut ()),
    context: *mut (),
  ) {
    self.hooks.push(LifecycleEntry {
      callback,
      context,
      name,
      free_context: false,
    });
  }

  /// Broadcast an event to all registered hooks.
  ///
  /// For closure-based hooks (registered via `register`), panics are caught
  /// inside the `extern "C"` trampoline and logged. Raw C hooks (registered
  /// via `register_raw`) must not panic — unwinding through an `extern "C"`
  /// frame is undefined behaviour.
  pub(crate) fn broadcast(&self, event: LifecycleEvent, meta: *const ExtensionMeta) {
    let event_name = match event {
      LifecycleEvent::OnLoad => "OnLoad",
      LifecycleEvent::OnUnload => "OnUnload",
      LifecycleEvent::OnReload => "OnReload",
    };
    for entry in &self.hooks {
      let hook_label = entry.name.as_deref().unwrap_or("<unnamed>");
      tracing::debug!(
        event = event_name,
        hook = hook_label,
        "broadcasting lifecycle event"
      );
      // SAFETY: entry.callback is a valid extern "C" fn pointer.
      // Closure-trampoline hooks catch panics internally.
      // Raw C hooks contractually must not panic.
      unsafe { (entry.callback)(meta, entry.context) };
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::ffi::CString;

  /// Verify that registering and broadcasting calls the hook.
  #[test]
  fn test_broadcast_calls_hook() {
    let mut reg = LifecycleRegister::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = std::sync::Arc::clone(&called);

    reg.register(
      Some("test_hook".to_string()),
      Box::new(move |_ptr| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
      }),
    );

    reg.broadcast(LifecycleEvent::OnLoad, std::ptr::null());
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
  }

  /// Verify that a panicking hook does not prevent subsequent hooks from running.
  #[test]
  fn test_panicking_hook_does_not_block() {
    let mut reg = LifecycleRegister::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = std::sync::Arc::clone(&called);

    reg.register(
      Some("panicker".to_string()),
      Box::new(|_| panic!("intentional panic")),
    );
    reg.register(
      Some("survivor".to_string()),
      Box::new(move |_ptr| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
      }),
    );

    // Dummy null meta — hooks in this test don't dereference it.
    reg.broadcast(LifecycleEvent::OnUnload, std::ptr::null());
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
  }
}
