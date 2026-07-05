//! Playback state types.

use serde::{Deserialize, Serialize};

/// Playback status for a stream.  Uses `#[repr(u8)]` for Atomic compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum PlaybackStatus {
  Playing = 0,
  Paused = 1,
  Stopped = 2,
  Buffering = 3,
  Error = 4,
}
