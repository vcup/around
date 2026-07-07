//! around-audio-sdk: Audio codec SDK crate (ADR-0004 §3) and Filter SDK.
//!
//! Defines the ABI-stable [`Codec`] trait and [`Filter`] trait with stabby
//! `#[slot]` dispatch, safe wrapper types, and startup registration functions.
//!
//! Extensions implementing the Codec or Filter trait depend on this crate.
//! The host binary calls [`init_codec`] and [`init_filter`] at startup to
//! register the slots with the framework.

pub mod codec;
pub mod filter;

// -- Primary re-exports (filter) --
// NOTE: NAME and REGISTER_VTABLE are NOT re-exported from filter at the top level
// because codec also exports them with the same names. Use `around_audio_sdk::filter::NAME`
// directly, or import with `use around_audio_sdk::filter::{self}`.
pub use filter::{
  init_filter, make_dyn_filter, AudioBufferC, DynFilterRef, Filter, FilterDyn, FilterRegister,
};
// -- Primary re-exports (codec) --
pub use codec::{
  init_codec, Codec, CodecDyn, CodecRegister, DynCodecRef, ReadFn, SafeCodecRef, SeekFn,
  StreamInfo, NAME, REGISTER_VTABLE,
};

/// SDK convenience re-export of `ExtensionMeta` so extension crates (wav,
/// test-pcm) need only one SDK dependency for all framework types.
pub use around_extensions::ExtensionMeta;

/// Compile-time assertion: slot NAME matches the expected prefix.
/// If this fails, existing compiled .so files will not load because the
/// create symbol changed.
#[test]
fn slot_name_stability() {
  assert_eq!(
    codec::NAME,
    "around_audio_sdk::Codec",
    "Changing slot NAME breaks create symbol compatibility with \
         existing .so codec files. Update #[slot(prefix = ...)] accordingly."
  );
}
