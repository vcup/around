//! around-engine: Playback pipeline, IPC server, extension manager, and platform backends.

pub mod config;
pub mod extensions;
pub mod ipc;
pub(crate) mod ipc_codec;
pub mod ipc_transport;
pub mod output;
pub mod pipeline;
pub mod platform;

pub use config::EngineConfig;
pub use extensions::{DecoderInfo, ExtensionManager};
pub use ipc::{run_ipc_server, IpcCommand, IpcResponse, PlaybackState};
pub use output::AudioOutput;
pub use pipeline::Engine;
