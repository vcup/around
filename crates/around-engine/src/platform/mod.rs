//! Platform-specific audio output backends.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::AudioOutput;
#[cfg(target_os = "macos")]
pub use macos::AudioOutput;
#[cfg(target_os = "windows")]
pub use windows::AudioOutput;

/// Common interface for platform audio output.
/// Re-exported from the active platform module.
///
/// Full cpal integration deferred to a later phase.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub struct AudioOutput {
  _spec: around_core::SampleSpec,
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl AudioOutput {
  pub fn new(spec: around_core::SampleSpec) -> Result<Self, String> {
    Ok(Self { _spec: spec })
  }

  pub fn default_device() -> Option<String> {
    None
  }
}
