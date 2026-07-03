//! Playback state types.

/// Playback status for a stream.  Uses `#[repr(u8)]` for Atomic compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaybackStatus {
  Playing = 0,
  Paused = 1,
  Stopped = 2,
  Buffering = 3,
  Error = 4,
}
