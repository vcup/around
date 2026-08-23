//! around-engine: Playback pipeline, IPC server, extension manager, and platform backends.

pub mod config;
pub mod cpal_adapter;
pub mod filter_chain;
pub mod ipc;
pub mod output;
pub mod output_plan;
pub mod output_session;
pub mod pcm_buffer;
pub mod pipeline;
pub mod platform;

pub use config::{EngineConfig, OutputDriver};
pub use ipc::run_ipc_server;
pub use ipc::types::{
  CodecDescriptor, ErrorCode, IpcCommand, IpcResponse, ResponseStatus, StreamStatus, TrackInfo,
};
pub use output_plan::{
  plan_output, DeviceSelector, DeviceSnapshot, ModePreference, OutputCapabilities, OutputCategory,
  OutputDevice, OutputMode, OutputPlan, OutputPreference, RecoveryPolicy, SampleRateRange,
};
pub use output_session::{
  BindingStatus, MemoryOutputAdapter, OutputBinding, OutputDiscovery, OutputError, OutputSession,
};
pub use pipeline::{Engine, PlaybackHandle, PreparedStream, SeekError, StreamId, StreamState};
