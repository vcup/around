//! around-engine: Playback pipeline, IPC server, extension manager, and platform backends.

pub mod config;
pub mod pipeline;
pub mod platform;

pub use config::EngineConfig;
pub use pipeline::Engine;
