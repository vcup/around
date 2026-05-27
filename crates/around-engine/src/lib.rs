//! around-engine: Playback pipeline, IPC server, extension manager, and platform backends.

pub mod ipc;

pub mod config;
pub mod pipeline;
pub mod platform;

pub use config::EngineConfig;
pub use ipc::{run_ipc_server, IpcCommand, IpcResponse, PlaybackState};
pub use pipeline::Engine;
