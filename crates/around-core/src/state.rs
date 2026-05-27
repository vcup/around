//! Playback state types.

pub type PlaybackStatus = u8;

pub const PLAYING: PlaybackStatus = 0;
pub const PAUSED: PlaybackStatus = 1;
pub const STOPPED: PlaybackStatus = 2;
pub const BUFFERING: PlaybackStatus = 3;
