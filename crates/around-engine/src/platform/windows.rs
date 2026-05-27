//! Windows audio output backend (WASAPI via cpal).
//! Stub — cpal integration deferred to full implementation.

use around_core::SampleSpec;

pub struct AudioOutput {
  _spec: SampleSpec,
}

impl AudioOutput {
  pub fn new(spec: SampleSpec) -> Result<Self, String> {
    Ok(Self { _spec: spec })
  }

  pub fn default_device() -> Option<String> {
    None
  }
}
