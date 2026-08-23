//! OutputSession and the CPAL-free output ports.
//!
//! OutputSession concentrates planning, FilterChain construction, exact-format
//! binding, and write ordering. `MemoryOutputAdapter` and the CPAL Adapter use
//! the same ports, so tests exercise the real Stream output lifecycle.

use crate::filter_chain::{FilterChain, FilterError};
use crate::output_plan::{plan_output, DeviceSnapshot, OutputPlan, OutputPreference, PlanError};
use crate::pcm_buffer::PcmBuffer;
use around_core::{AudioBufferError, AudioBufferRef, PcmEncoding, SampleSpec};
use parking_lot::Mutex;
use std::fmt;
use std::sync::Arc;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingStatus {
  Active,
  Lost,
  Closed,
}

pub trait OutputDiscovery: Send + Sync {
  fn snapshot(&self) -> Result<DeviceSnapshot, OutputError>;
}

pub trait OutputBinding: Send {
  fn device_id(&self) -> &str;
  fn spec(&self) -> SampleSpec;
  fn status(&self) -> BindingStatus;
  fn write(&mut self, block: &AudioBufferRef<'_>) -> Result<(), OutputError>;
  fn close(&mut self) -> Result<(), OutputError>;
}

pub trait OutputBinder: Send + Sync {
  fn bind(&self, plan: &OutputPlan) -> Result<Box<dyn OutputBinding>, OutputError>;
}

#[derive(Debug)]
pub enum OutputError {
  Audio(AudioBufferError),
  Discovery(String),
  Planning(PlanError),
  Binding(String),
  Write(String),
  Closed,
  Lost,
}
impl fmt::Display for OutputError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Audio(error) => error.fmt(f),
      Self::Discovery(message) => write!(f, "output discovery failed: {message}"),
      Self::Planning(error) => error.fmt(f),
      Self::Binding(message) => write!(f, "output binding failed: {message}"),
      Self::Write(message) => write!(f, "output write failed: {message}"),
      Self::Closed => f.write_str("output binding is closed"),
      Self::Lost => f.write_str("output device is lost"),
    }
  }
}

impl std::error::Error for OutputError {}

impl From<PlanError> for OutputError {
  fn from(error: PlanError) -> Self {
    Self::Planning(error)
  }
}
impl From<AudioBufferError> for OutputError {
  fn from(error: AudioBufferError) -> Self {
    Self::Audio(error)
  }
}

impl From<FilterError> for OutputError {
  fn from(error: FilterError) -> Self {
    Self::Write(error.to_string())
  }
}

pub struct OutputSession {
  discovery: Box<dyn OutputDiscovery>,
  binder: Box<dyn OutputBinder>,
  preferences: Vec<OutputPreference>,
  plan: OutputPlan,
  chain: FilterChain,
  binding: Box<dyn OutputBinding>,
}

impl OutputSession {
  pub fn open(
    discovery: Box<dyn OutputDiscovery>,
    binder: Box<dyn OutputBinder>,
    source: SampleSpec,
    filters: Vec<Box<dyn crate::filter_chain::Filter>>,
    preferences: Vec<OutputPreference>,
  ) -> Result<Self, OutputError> {
    let snapshot = discovery.snapshot()?;
    let plan = plan_output(&snapshot, source, &preferences)?;
    let chain = FilterChain::build(source, filters, plan.target_spec);
    let binding = binder.bind(&plan)?;
    if binding.spec() != plan.target_spec || binding.device_id() != plan.device_id {
      return Err(OutputError::Binding(
        "adapter returned a binding different from its plan".into(),
      ));
    }
    Ok(Self {
      discovery,
      binder,
      preferences,
      plan,
      chain,
      binding,
    })
  }

  pub fn submit(
    &mut self,
    decoded: &[f32],
    frames: usize,
    workspace: &mut PcmBuffer,
  ) -> Result<usize, OutputError> {
    match self.binding.status() {
      BindingStatus::Active => {}
      BindingStatus::Lost => return Err(OutputError::Lost),
      BindingStatus::Closed => return Err(OutputError::Closed),
    }
    let written = self.chain.process_to_output(decoded, frames, workspace)?;
    let output_bytes = workspace.output_region();
    let frame_bytes = self.plan.target_spec.bytes_per_frame();
    let block = AudioBufferRef::new(
      self.plan.target_spec,
      written / frame_bytes,
      &output_bytes[..written],
    )?;
    self.binding.write(&block)?;
    Ok(written)
  }

  pub fn submit_workspace(
    &mut self,
    workspace: &mut PcmBuffer,
    frames: usize,
  ) -> Result<usize, OutputError> {
    match self.binding.status() {
      BindingStatus::Active => {}
      BindingStatus::Lost => return Err(OutputError::Lost),
      BindingStatus::Closed => return Err(OutputError::Closed),
    }
    let written = self.chain.process_workspace(workspace, frames)?;
    let output_bytes = workspace.output_region();
    let frame_bytes = self.plan.target_spec.bytes_per_frame();
    let block = AudioBufferRef::new(
      self.plan.target_spec,
      written / frame_bytes,
      &output_bytes[..written],
    )?;
    self.binding.write(&block)?;
    Ok(written)
  }

  pub fn target_spec(&self) -> SampleSpec {
    self.plan.target_spec
  }

  pub fn plan(&self) -> &OutputPlan {
    &self.plan
  }

  pub fn preferences(&self) -> &[OutputPreference] {
    &self.preferences
  }

  pub fn chain(&self) -> &FilterChain {
    &self.chain
  }

  /// Rebuild the plan and binding from a fresh capability snapshot. The old
  /// binding is retained until the new plan and binding both succeed.
  pub fn rebind(&mut self) -> Result<(), OutputError> {
    let snapshot = self.discovery.snapshot()?;
    let source = self.chain.source_spec();
    let plan = plan_output(&snapshot, source, &self.preferences)?;
    let binding = self.binder.bind(&plan)?;
    if binding.spec() != plan.target_spec || binding.device_id() != plan.device_id {
      return Err(OutputError::Binding(
        "adapter returned a binding different from its plan".into(),
      ));
    }
    let target_spec = plan.target_spec;
    self.binding.close()?;
    self.binding = binding;
    self.chain.reconfigure_output(target_spec);
    self.plan = plan;
    Ok(())
  }

  pub fn workspace_requirements(&self, input_samples: usize) -> (usize, usize) {
    self.chain.workspace_requirements(input_samples)
  }
}

impl Drop for OutputSession {
  fn drop(&mut self) {
    let _ = self.binding.close();
  }
}

#[derive(Clone)]
pub struct MemoryOutputAdapter {
  snapshot: DeviceSnapshot,
  writes: Arc<Mutex<Vec<Vec<u8>>>>,
  lost: Arc<Mutex<bool>>,
  discard: bool,
}

impl MemoryOutputAdapter {
  pub fn new(snapshot: DeviceSnapshot) -> Self {
    Self {
      snapshot,
      writes: Arc::new(Mutex::new(Vec::new())),
      lost: Arc::new(Mutex::new(false)),
      discard: false,
    }
  }

  pub fn discard(snapshot: DeviceSnapshot) -> Self {
    Self {
      snapshot,
      writes: Arc::new(Mutex::new(Vec::new())),
      lost: Arc::new(Mutex::new(false)),
      discard: true,
    }
  }

  pub fn writes(&self) -> Arc<Mutex<Vec<Vec<u8>>>> {
    Arc::clone(&self.writes)
  }

  pub fn set_lost(&self, lost: bool) {
    *self.lost.lock() = lost;
  }
}

impl OutputDiscovery for MemoryOutputAdapter {
  fn snapshot(&self) -> Result<DeviceSnapshot, OutputError> {
    Ok(self.snapshot.clone())
  }
}

impl OutputBinder for MemoryOutputAdapter {
  fn bind(&self, plan: &OutputPlan) -> Result<Box<dyn OutputBinding>, OutputError> {
    if plan.snapshot_revision != self.snapshot.revision {
      return Err(OutputError::Binding(
        "output capabilities changed since planning".into(),
      ));
    }
    let device = self
      .snapshot
      .devices
      .iter()
      .find(|device| device.id == plan.device_id)
      .ok_or_else(|| OutputError::Binding("planned device disappeared".into()))?;
    let mode = device
      .capabilities
      .modes
      .iter()
      .find(|mode| mode.id == plan.mode_id)
      .ok_or_else(|| OutputError::Binding("planned mode disappeared".into()))?;
    if mode
      .spec_at(plan.target_spec.sample_rate)
      .map_err(|error| OutputError::Binding(error.to_string()))?
      != plan.target_spec
    {
      return Err(OutputError::Binding("planned output mode changed".into()));
    }
    Ok(Box::new(MemoryOutputBinding {
      device_id: plan.device_id.clone(),
      spec: plan.target_spec,
      writes: Arc::clone(&self.writes),
      lost: Arc::clone(&self.lost),
      closed: false,
      discard: self.discard,
    }))
  }
}

struct MemoryOutputBinding {
  device_id: String,
  spec: SampleSpec,
  writes: Arc<Mutex<Vec<Vec<u8>>>>,
  lost: Arc<Mutex<bool>>,
  closed: bool,
  discard: bool,
}

impl OutputBinding for MemoryOutputBinding {
  fn device_id(&self) -> &str {
    &self.device_id
  }

  fn spec(&self) -> SampleSpec {
    self.spec
  }

  fn status(&self) -> BindingStatus {
    if self.closed {
      BindingStatus::Closed
    } else if *self.lost.lock() {
      BindingStatus::Lost
    } else {
      BindingStatus::Active
    }
  }

  fn write(&mut self, block: &AudioBufferRef<'_>) -> Result<(), OutputError> {
    match self.status() {
      BindingStatus::Active => {}
      BindingStatus::Lost => return Err(OutputError::Lost),
      BindingStatus::Closed => return Err(OutputError::Closed),
    }
    let bytes = block.data();
    if block.spec() != self.spec || bytes.len() % self.spec.bytes_per_frame() != 0 {
      return Err(OutputError::Write(
        "output block does not match binding".into(),
      ));
    }
    if !self.discard {
      self.writes.lock().push(bytes.to_vec());
    }
    Ok(())
  }

  fn close(&mut self) -> Result<(), OutputError> {
    self.closed = true;
    Ok(())
  }
}

/// A virtual F32 output used by `OutputDriver::Null`.
pub fn null_adapter(source: SampleSpec) -> MemoryOutputAdapter {
  let device = crate::output_plan::OutputDevice {
    id: "null".into(),
    name: "Null Output".into(),
    category: crate::output_plan::OutputCategory::Other,
    priority: i16::MAX,
    capabilities: crate::output_plan::OutputCapabilities {
      modes: vec![crate::output_plan::OutputMode {
        id: 0,
        channels: u16::from(source.channels),
        encoding: PcmEncoding::F32,
        rates: crate::output_plan::SampleRateRange {
          min: source.sample_rate,
          max: source.sample_rate,
        },
        default_rate: source.sample_rate,
        is_default: true,
        quality_rank: u16::MAX,
      }],
    },
  };
  MemoryOutputAdapter::discard(crate::output_plan::DeviceSnapshot {
    revision: 1,
    devices: vec![device],
  })
}

#[cfg(test)]
mod tests {
  #![expect(
    clippy::unwrap_used,
    reason = "output session tests use validated in-memory adapters"
  )]
  use super::*;
  use crate::output_plan::{
    DeviceSelector, ModePreference, OutputCapabilities, OutputCategory, OutputDevice, OutputMode,
    RecoveryPolicy, SampleRateRange,
  };
  use around_core::PcmEncoding;
  use std::collections::VecDeque;
  use std::sync::Arc;

  #[derive(Clone)]
  struct SnapshotDiscovery {
    current: Arc<Mutex<DeviceSnapshot>>,
    next: Arc<Mutex<VecDeque<DeviceSnapshot>>>,
  }

  impl SnapshotDiscovery {
    fn new(mut snapshots: Vec<DeviceSnapshot>) -> Self {
      let current = snapshots.remove(0);
      Self {
        current: Arc::new(Mutex::new(current)),
        next: Arc::new(Mutex::new(snapshots.into_iter().collect())),
      }
    }
  }

  impl OutputDiscovery for SnapshotDiscovery {
    fn snapshot(&self) -> Result<DeviceSnapshot, OutputError> {
      let current = self.current.lock().clone();
      if let Some(next) = self.next.lock().pop_front() {
        *self.current.lock() = next;
      }
      Ok(current)
    }
  }

  fn snapshot_with_revision(revision: u64) -> DeviceSnapshot {
    DeviceSnapshot {
      revision,
      devices: vec![OutputDevice {
        id: "memory".into(),
        name: "Memory".into(),
        category: OutputCategory::Other,
        priority: 1,
        capabilities: OutputCapabilities {
          modes: vec![OutputMode {
            id: 1,
            channels: 2,
            encoding: PcmEncoding::I16,
            rates: SampleRateRange {
              min: 44100,
              max: 48000,
            },
            default_rate: 48000,
            is_default: true,
            quality_rank: 1,
          }],
        },
      }],
    }
  }

  fn adapter() -> MemoryOutputAdapter {
    MemoryOutputAdapter::new(snapshot_with_revision(1))
  }

  fn preferences() -> Vec<OutputPreference> {
    vec![OutputPreference {
      selector: DeviceSelector::Any,
      mode: ModePreference::Any,
      recovery: RecoveryPolicy::Stop,
    }]
  }
  #[test]
  fn session_writes_exact_target_bytes() {
    let adapter = adapter();
    let writes = adapter.writes();
    let source = SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap();
    let mut session = OutputSession::open(
      Box::new(adapter.clone()),
      Box::new(adapter),
      source,
      vec![],
      preferences(),
    )
    .unwrap();
    let mut workspace = PcmBuffer::new(8, 128);
    session
      .submit(&[1.0, -1.0, 0.5, -0.5], 2, &mut workspace)
      .unwrap();
    assert_eq!(writes.lock().len(), 1);
    assert_eq!(writes.lock()[0].len(), 8);
    assert_eq!(session.target_spec().encoding, PcmEncoding::I16);
  }

  #[test]
  fn lost_binding_does_not_replay_failed_block() {
    let adapter = adapter();
    let writes = adapter.writes();
    let source = SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap();
    let mut session = OutputSession::open(
      Box::new(adapter.clone()),
      Box::new(adapter.clone()),
      source,
      vec![],
      preferences(),
    )
    .unwrap();
    let mut workspace = PcmBuffer::new(8, 128);
    session
      .submit(&[1.0, -1.0, 0.5, -0.5], 2, &mut workspace)
      .unwrap();
    assert_eq!(writes.lock().len(), 1);

    adapter.set_lost(true);
    assert!(matches!(
      session.submit(&[0.25, -0.25, 0.125, -0.125], 2, &mut workspace),
      Err(OutputError::Lost)
    ));
    adapter.set_lost(false);
    session.rebind().unwrap();
    session.submit(&[0.0; 4], 2, &mut workspace).unwrap();

    assert_eq!(writes.lock().len(), 2);
  }
  #[test]
  fn binding_rejects_mismatched_buffer_and_writes_after_close() {
    let source = SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap();
    let snapshot = snapshot_with_revision(1);
    let plan = plan_output(&snapshot, source, &preferences()).unwrap();
    let adapter = MemoryOutputAdapter::new(snapshot);
    let mut binding = adapter.bind(&plan).unwrap();

    let mismatched = AudioBufferRef::new(source, 1, &[0; 8]).unwrap();
    assert!(matches!(
      binding.write(&mismatched),
      Err(OutputError::Write(message)) if message.contains("match")
    ));

    let target = AudioBufferRef::new(plan.target_spec, 1, &[0; 4]).unwrap();
    binding.close().unwrap();
    assert_eq!(binding.status(), BindingStatus::Closed);
    assert!(matches!(binding.write(&target), Err(OutputError::Closed)));
    binding.close().unwrap();
  }

  #[test]
  fn binding_rejects_stale_plan_revision() {
    let source = SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap();
    let planned_snapshot = snapshot_with_revision(1);
    let plan = plan_output(&planned_snapshot, source, &preferences()).unwrap();
    let stale_adapter = MemoryOutputAdapter::new(snapshot_with_revision(2));

    assert!(matches!(
      stale_adapter.bind(&plan),
      Err(OutputError::Binding(message)) if message.contains("capabilities changed")
    ));
  }

  #[test]
  fn failed_rebind_keeps_old_binding_active_until_replacement_is_ready() {
    let source = SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap();
    let binder = adapter();
    let writes = binder.writes();
    let discovery =
      SnapshotDiscovery::new(vec![snapshot_with_revision(1), snapshot_with_revision(2)]);
    let mut session = OutputSession::open(
      Box::new(discovery),
      Box::new(binder),
      source,
      vec![],
      preferences(),
    )
    .unwrap();
    let mut workspace = PcmBuffer::new(8, 128);
    session.submit(&[0.0; 4], 2, &mut workspace).unwrap();

    assert!(matches!(
      session.rebind(),
      Err(OutputError::Binding(message)) if message.contains("capabilities changed")
    ));
    session.submit(&[0.5; 4], 2, &mut workspace).unwrap();
    assert_eq!(writes.lock().len(), 2);
  }

  #[test]
  fn null_adapter_discards_but_validates_format() {
    let source = SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap();
    let adapter = null_adapter(source);
    let mut session = OutputSession::open(
      Box::new(adapter),
      Box::new(null_adapter(source)),
      source,
      vec![],
      preferences_for_null(),
    )
    .unwrap();
    let mut workspace = PcmBuffer::new(8, 128);
    assert!(session.submit(&[0.0; 4], 2, &mut workspace).is_ok());
  }

  fn preferences_for_null() -> Vec<OutputPreference> {
    vec![OutputPreference {
      selector: DeviceSelector::Exact("null".into()),
      mode: ModePreference::Any,
      recovery: RecoveryPolicy::Stop,
    }]
  }
}
