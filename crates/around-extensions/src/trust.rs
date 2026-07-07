//! Progressive trust model for extension operations.
//!
//! Every loaded extension has a [`TrustCounter`] tracking consecutive successful
//! guarded operations. After N successes (default 100), the extension is
//! considered [`Trusted`](TrustLevel::Trusted) and guards may be elided on
//! hot paths. A crash during a guarded operation resets the counter.
//!
//! The trust model supports two granularities:
//! - **Per-extension**: a single counter for all operations of an extension.
//! - **Per-Register**: separate counters keyed by slot name, so one Register's
//!   operations don't contaminate another's trust state.
//!
//! See also: `.evolution/gen-002/design-extension-safety.md` §1

use std::sync::atomic::{AtomicU32, Ordering};

/// The trust level of an extension, based on consecutive successful operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustLevel {
  /// Freshly loaded or recent crash — all operations are guarded.
  Untrusted,
  /// Has accumulated some successes but has not yet reached the required
  /// threshold to be trusted.
  Probation {
    /// Consecutive successes so far.
    successes: u32,
    /// Number of successes required to reach Trusted.
    required: u32,
  },
  /// Proven reliable — guard elision is eligible.
  Trusted,
}

/// Per-Register counter of consecutive successful guarded operations.
///
/// Starts at 0 (untrusted). After `required` consecutive successful operations
/// the register graduates to `Trusted`. A crash resets the counter to 0.
///
/// Thread-safe via atomics: relaxed ordering is sufficient because the guard
/// check is advisory — a stale read means we guard one extra call (conservative)
/// or skip one guard (benign, since the real guard is always present until
/// Trusted).
pub(crate) struct TrustCounter {
  /// Consecutive successful operations. Saturating.
  successes: AtomicU32,
  /// Number of consecutive successes needed to reach `Trusted`.
  required: u32,
}

impl TrustCounter {
  /// Create a new counter with the given required-success threshold.
  ///
  /// The register starts untrusted. After `required` calls to
  /// [`record_success`](Self::record_success) without a reset, it becomes
  /// [`Trusted`](TrustLevel::Trusted).
  pub(crate) fn new(required: u32) -> Self {
    Self {
      successes: AtomicU32::new(0),
      required,
    }
  }

  /// Record one successful guarded operation.
  ///
  /// Increments the internal counter and returns the current [`TrustLevel`]
  /// based on the new count. Once the counter reaches `required`, the level
  /// becomes [`Trusted`](TrustLevel::Trusted).
  ///
  /// Between `Untrusted` and `Trusted` the level is
  /// [`Probation`](TrustLevel::Probation), reporting the current count and
  /// the required threshold.
  pub(crate) fn record_success(&self) -> TrustLevel {
    let prev = self.successes.fetch_add(1, Ordering::Relaxed);
    let current = prev + 1;
    if current >= self.required {
      TrustLevel::Trusted
    } else {
      TrustLevel::Probation {
        successes: current,
        required: self.required,
      }
    }
  }

  /// Record an operation outcome for this Register.
  ///
  /// - `true`: increments the success counter.
  /// - `false`: resets the counter to 0 (crash or error).
  ///
  /// Returns the new [`TrustLevel`].
  pub(crate) fn record_guarded_outcome(&self, success: bool) -> TrustLevel {
    if success {
      self.record_success()
    } else {
      self.reset();
      TrustLevel::Untrusted
    }
  }

  /// Returns `true` if the counter has reached the required threshold.
  ///
  /// When `true`, guard elision is eligible for this Register's operations.
  pub(crate) fn is_trusted(&self) -> bool {
    self.successes.load(Ordering::Relaxed) >= self.required
  }

  /// Reset the counter (e.g. after a crash during a guarded operation).
  ///
  /// The register returns to [`Untrusted`](TrustLevel::Untrusted) and must
  /// re-earn trust from zero.
  pub(crate) fn reset(&self) {
    self.successes.store(0, Ordering::Relaxed);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_fresh_counter_is_untrusted() {
    let c = TrustCounter::new(100);
    assert!(!c.is_trusted());
  }

  #[test]
  fn test_after_required_successes_is_trusted() {
    let c = TrustCounter::new(3);
    assert_eq!(
      c.record_success(),
      TrustLevel::Probation {
        successes: 1,
        required: 3
      }
    );
    assert!(!c.is_trusted());
    assert_eq!(
      c.record_success(),
      TrustLevel::Probation {
        successes: 2,
        required: 3
      }
    );
    assert!(!c.is_trusted());
    assert_eq!(c.record_success(), TrustLevel::Trusted);
    assert!(c.is_trusted());
  }

  #[test]
  fn test_reset_returns_to_untrusted() {
    let c = TrustCounter::new(3);
    let _ = c.record_success();
    let _ = c.record_success();
    c.reset();
    assert!(!c.is_trusted());
    // Next success is Probation again
    assert_eq!(
      c.record_success(),
      TrustLevel::Probation {
        successes: 1,
        required: 3
      }
    );
  }

  #[test]
  fn test_default_required_stays_untrusted_until_threshold() {
    let c = TrustCounter::new(100);
    for i in 1..100 {
      match c.record_success() {
        TrustLevel::Probation {
          successes,
          required,
        } => {
          assert_eq!(successes, i);
          assert_eq!(required, 100);
        }
        other => panic!("expected Probation at step {i}, got {other:?}"),
      }
      assert!(!c.is_trusted());
    }
    assert_eq!(c.record_success(), TrustLevel::Trusted);
    assert!(c.is_trusted());
  }

  #[test]
  fn test_trusted_remains_trusted() {
    let c = TrustCounter::new(1);
    assert_eq!(c.record_success(), TrustLevel::Trusted);
    assert!(c.is_trusted());
    // Further successes keep it trusted
    assert_eq!(c.record_success(), TrustLevel::Trusted);
    assert!(c.is_trusted());
  }

  #[test]
  fn test_record_guarded_outcome_success() {
    let c = TrustCounter::new(3);
    assert_eq!(
      c.record_guarded_outcome(true),
      TrustLevel::Probation {
        successes: 1,
        required: 3
      }
    );
  }

  #[test]
  fn test_record_guarded_outcome_failure_resets() {
    let c = TrustCounter::new(3);
    let _ = c.record_success();
    let _ = c.record_success();
    assert_eq!(c.record_guarded_outcome(false), TrustLevel::Untrusted);
    assert!(!c.is_trusted());
  }
}
