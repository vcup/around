//! around-engine: Playback pipeline, IPC server, extension manager, and platform backends.

pub mod config;
pub mod filter_chain;
pub mod ipc;
pub mod output;
pub mod pcm_buffer;
pub mod pipeline;
pub mod platform;

pub use config::EngineConfig;
pub use ipc::run_ipc_server;
pub use ipc::types::{
  CodecDescriptor, ErrorCode, IpcCommand, IpcResponse, PlaybackState, ResponseStatus, TrackInfo,
  TrackState,
};
pub use pipeline::Engine;
