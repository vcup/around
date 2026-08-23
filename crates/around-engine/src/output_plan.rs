//! Pure OutputDevice capability modeling and output planning.
//!
//! This Module has no CPAL, OS, locks, clocks, or handles. Adapters turn their
//! native capabilities into a [`DeviceSnapshot`]; the planner selects one
//! exact native mode for a Stream. Format conversion remains the FilterChain's
//! responsibility.

use around_core::{ByteOrder, Interleave, PcmEncoding, SampleSpec};
use std::cmp::Ordering;
use std::fmt;

pub type OutputDeviceId = String;
pub type OutputModeId = u32;
pub type SnapshotRevision = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum OutputCategory {
  Bluetooth,
  Usb,
  Onboard,
  Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SampleRateRange {
  pub min: u32,
  pub max: u32,
}

impl SampleRateRange {
  pub fn new(min: u32, max: u32) -> Result<Self, PlanError> {
    if min == 0 || max < min {
      return Err(PlanError::InvalidCapabilities(
        "sample-rate range is empty or contains zero".into(),
      ));
    }
    Ok(Self { min, max })
  }

  pub const fn contains(self, rate: u32) -> bool {
    rate >= self.min && rate <= self.max
  }

  fn candidates(self, source_rate: u32, requested: Option<u32>, default_rate: u32) -> Vec<u32> {
    let mut rates = Vec::with_capacity(4);
    let mut add = |rate: u32| {
      if self.contains(rate) && !rates.contains(&rate) {
        rates.push(rate);
      }
    };
    add(source_rate);
    if let Some(rate) = requested {
      add(rate);
    }
    add(default_rate);
    add(self.min);
    add(self.max);
    rates
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputMode {
  pub id: OutputModeId,
  pub channels: u16,
  pub encoding: PcmEncoding,
  pub rates: SampleRateRange,
  pub default_rate: u32,
  pub is_default: bool,
  pub quality_rank: u16,
}

impl OutputMode {
  pub fn spec_at(&self, rate: u32) -> Result<SampleSpec, PlanError> {
    if !self.rates.contains(rate) {
      return Err(PlanError::NoMode(format!(
        "mode {} does not accept {rate} Hz",
        self.id
      )));
    }
    let channels = u8::try_from(self.channels).map_err(|_| PlanError::UnsupportedChannels {
      channels: self.channels,
    })?;
    SampleSpec::new(
      rate,
      channels,
      self.encoding,
      Interleave::Interleaved,
      ByteOrder::Native,
    )
    .map_err(|message| PlanError::InvalidCapabilities(message.into()))
  }

  pub fn accepts(&self, spec: SampleSpec) -> bool {
    self.channels == u16::from(spec.channels)
      && self.encoding == spec.encoding
      && spec.interleave == Interleave::Interleaved
      && spec.byte_order.is_native()
      && self.rates.contains(spec.sample_rate)
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputCapabilities {
  pub modes: Vec<OutputMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDevice {
  pub id: OutputDeviceId,
  pub name: String,
  pub category: OutputCategory,
  pub priority: i16,
  pub capabilities: OutputCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSnapshot {
  pub revision: SnapshotRevision,
  pub devices: Vec<OutputDevice>,
}

impl DeviceSnapshot {
  pub fn validate(&self) -> Result<(), PlanError> {
    for pair in self.devices.windows(2) {
      if pair[0].id >= pair[1].id {
        return Err(PlanError::InvalidCapabilities(
          "device snapshot ids must be unique and sorted".into(),
        ));
      }
    }
    for device in &self.devices {
      if device.id.is_empty() {
        return Err(PlanError::InvalidCapabilities(
          "device id must not be empty".into(),
        ));
      }
      for mode in &device.capabilities.modes {
        if mode.channels == 0 || mode.rates.min == 0 || mode.rates.max < mode.rates.min {
          return Err(PlanError::InvalidCapabilities(format!(
            "device {} has invalid mode {}",
            device.id, mode.id
          )));
        }
      }
    }
    Ok(())
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelector {
  Exact(OutputDeviceId),
  Name(String),
  Category(OutputCategory),
  Any,
}

impl DeviceSelector {
  fn matches(&self, device: &OutputDevice) -> bool {
    match self {
      Self::Exact(id) => &device.id == id,
      Self::Name(name) => &device.name == name,
      Self::Category(category) => &device.category == category,
      Self::Any => true,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModePreference {
  Any,
  Rate(u32),
  Encoding(PcmEncoding),
  Exact {
    rate: u32,
    encoding: PcmEncoding,
    channels: u8,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPolicy {
  ContinueOnFallback,
  PauseOnFallback,
  Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPreference {
  pub selector: DeviceSelector,
  pub mode: ModePreference,
  pub recovery: RecoveryPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionLoss {
  pub rate: u32,
  pub channels: u32,
  pub interleave: u32,
  pub encoding: u32,
  pub byte_order: u32,
}

impl ConversionLoss {
  pub const fn total(self) -> u32 {
    self
      .rate
      .saturating_add(self.channels)
      .saturating_add(self.interleave)
      .saturating_add(self.encoding)
      .saturating_add(self.byte_order)
  }
}

pub fn conversion_loss(input: SampleSpec, output: SampleSpec) -> ConversionLoss {
  let rate = if input.sample_rate > output.sample_rate {
    u32::try_from(u64::from(input.sample_rate - output.sample_rate) * u64::from(100_u32) / 1000)
      .unwrap_or(u32::MAX)
  } else {
    u32::try_from(u64::from(output.sample_rate - input.sample_rate) * u64::from(10_u32) / 1000)
      .unwrap_or(u32::MAX)
  };
  let channels = if input.channels > output.channels {
    u32::from(input.channels - output.channels) * 80
  } else {
    u32::from(output.channels - input.channels) * 5
  };
  let interleave = u32::from(input.interleave != output.interleave);
  let encoding = encoding_loss(input.encoding, output.encoding);
  let byte_order = u32::from(input.byte_order != output.byte_order);
  ConversionLoss {
    rate,
    channels,
    interleave,
    encoding,
    byte_order,
  }
}

fn encoding_loss(input: PcmEncoding, output: PcmEncoding) -> u32 {
  if input == output {
    return 0;
  }
  let bits = u32::from(output.bits().abs_diff(input.bits()));
  if input.is_float() != output.is_float() || input.is_signed() != output.is_signed() {
    return 20 + bits;
  }
  bits
}

pub struct OutputPlan {
  pub snapshot_revision: SnapshotRevision,
  pub device_id: OutputDeviceId,
  pub mode_id: OutputModeId,
  pub target_spec: SampleSpec,
  pub preference_index: usize,
  pub device_priority: i16,
  pub quality_rank: u16,
  pub is_default: bool,
  pub conversion_loss: ConversionLoss,
  pub recovery: RecoveryPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
  NoDevice(String),
  NoMode(String),
  UnsupportedChannels { channels: u16 },
  InvalidPreference(String),
  InvalidCapabilities(String),
}

impl fmt::Display for PlanError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::NoDevice(message) => write!(f, "no output device: {message}"),
      Self::NoMode(message) => write!(f, "no output mode: {message}"),
      Self::UnsupportedChannels { channels } => write!(f, "unsupported channels: {channels}"),
      Self::InvalidPreference(message) => write!(f, "invalid output preference: {message}"),
      Self::InvalidCapabilities(message) => write!(f, "invalid output capabilities: {message}"),
    }
  }
}

impl std::error::Error for PlanError {}

pub fn plan_output(
  snapshot: &DeviceSnapshot,
  source_spec: SampleSpec,
  preferences: &[OutputPreference],
) -> Result<OutputPlan, PlanError> {
  snapshot.validate()?;
  if preferences.is_empty() {
    return Err(PlanError::InvalidPreference(
      "at least one output preference is required".into(),
    ));
  }

  let mut best: Option<OutputPlan> = None;
  for (preference_index, preference) in preferences.iter().enumerate() {
    for device in &snapshot.devices {
      if !preference.selector.matches(device) {
        continue;
      }
      for mode in &device.capabilities.modes {
        let requested_rate = match preference.mode {
          ModePreference::Rate(rate) => Some(rate),
          ModePreference::Exact { rate, .. } => Some(rate),
          ModePreference::Any | ModePreference::Encoding(_) => None,
        };
        if let ModePreference::Encoding(encoding) | ModePreference::Exact { encoding, .. } =
          preference.mode
        {
          if mode.encoding != encoding {
            continue;
          }
        }
        if let ModePreference::Exact { rate, channels, .. } = preference.mode {
          if mode.channels != u16::from(channels) || !mode.rates.contains(rate) {
            continue;
          }
        }
        let rates = if let ModePreference::Exact { rate, .. } = preference.mode {
          vec![rate]
        } else {
          mode
            .rates
            .candidates(source_spec.sample_rate, requested_rate, mode.default_rate)
        };
        for rate in rates {
          let target_spec = mode.spec_at(rate)?;
          let loss = conversion_loss(source_spec, target_spec);
          let candidate = OutputPlan {
            snapshot_revision: snapshot.revision,
            device_id: device.id.clone(),
            mode_id: mode.id,
            target_spec,
            preference_index,
            device_priority: device.priority,
            quality_rank: mode.quality_rank,
            is_default: mode.is_default,
            conversion_loss: loss,
            recovery: preference.recovery,
          };
          if best
            .as_ref()
            .is_none_or(|current| compare_plan(&candidate, current) == Ordering::Less)
          {
            best = Some(candidate);
          }
        }
      }
    }
  }
  best.ok_or_else(|| PlanError::NoDevice("no preference matched a supported mode".into()))
}

fn compare_plan(left: &OutputPlan, right: &OutputPlan) -> Ordering {
  (
    left.preference_index,
    std::cmp::Reverse(left.device_priority),
    left.conversion_loss.total(),
    std::cmp::Reverse(left.quality_rank),
    std::cmp::Reverse(left.is_default),
    std::cmp::Reverse(left.target_spec.sample_rate),
    left.device_id.as_str(),
    left.mode_id,
  )
    .cmp(&(
      right.preference_index,
      std::cmp::Reverse(right.device_priority),
      right.conversion_loss.total(),
      std::cmp::Reverse(right.quality_rank),
      std::cmp::Reverse(right.is_default),
      std::cmp::Reverse(right.target_spec.sample_rate),
      right.device_id.as_str(),
      right.mode_id,
    ))
}

#[cfg(test)]
mod tests {
  #![expect(clippy::unwrap_used)]
  use super::*;

  fn device(id: &str, priority: i16, mode: OutputMode) -> OutputDevice {
    OutputDevice {
      id: id.into(),
      name: id.into(),
      category: OutputCategory::Other,
      priority,
      capabilities: OutputCapabilities { modes: vec![mode] },
    }
  }

  fn mode(id: u32, rate: u32, encoding: PcmEncoding) -> OutputMode {
    OutputMode {
      id,
      channels: 2,
      encoding,
      rates: SampleRateRange {
        min: rate,
        max: rate,
      },
      default_rate: rate,
      is_default: true,
      quality_rank: 1,
    }
  }

  fn any() -> OutputPreference {
    OutputPreference {
      selector: DeviceSelector::Any,
      mode: ModePreference::Any,
      recovery: RecoveryPolicy::ContinueOnFallback,
    }
  }

  #[test]
  fn preference_rank_beats_conversion_loss() {
    let snapshot = DeviceSnapshot {
      revision: 1,
      devices: vec![
        device("a", 0, mode(1, 44100, PcmEncoding::F32)),
        device("b", 0, mode(1, 48000, PcmEncoding::F32)),
      ],
    };
    let first = OutputPreference {
      selector: DeviceSelector::Exact("a".into()),
      ..any()
    };
    let plan = plan_output(
      &snapshot,
      SampleSpec::interleaved(48000, 2, PcmEncoding::F32).unwrap(),
      &[first, any()],
    )
    .unwrap();
    assert_eq!(plan.device_id, "a");
    assert_eq!(plan.preference_index, 0);
  }

  #[test]
  fn priority_beats_conversion_loss_within_preference() {
    let snapshot = DeviceSnapshot {
      revision: 1,
      devices: vec![
        device("a", 2, mode(1, 44100, PcmEncoding::F32)),
        device("b", 1, mode(1, 48000, PcmEncoding::F32)),
      ],
    };
    let plan = plan_output(
      &snapshot,
      SampleSpec::interleaved(48000, 2, PcmEncoding::F32).unwrap(),
      &[any()],
    )
    .unwrap();
    assert_eq!(plan.device_id, "a");
  }

  #[test]
  fn exact_preference_rejects_wrong_mode() {
    let snapshot = DeviceSnapshot {
      revision: 1,
      devices: vec![device("a", 0, mode(1, 44100, PcmEncoding::F32))],
    };
    let preference = OutputPreference {
      selector: DeviceSelector::Any,
      mode: ModePreference::Exact {
        rate: 48000,
        encoding: PcmEncoding::F32,
        channels: 2,
      },
      recovery: RecoveryPolicy::Stop,
    };
    assert!(plan_output(
      &snapshot,
      SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap(),
      &[preference],
    )
    .is_err());
  }
  #[test]
  fn mode_accepts_only_exact_native_interleaved_spec() {
    let mode = mode(1, 48000, PcmEncoding::F32);
    assert!(mode.accepts(SampleSpec::interleaved(48000, 2, PcmEncoding::F32).unwrap()));
    assert!(!mode.accepts(
      SampleSpec::new(
        48000,
        2,
        PcmEncoding::F32,
        Interleave::Planar,
        ByteOrder::Native,
      )
      .unwrap()
    ));
  }
  #[test]
  fn byte_order_loss_is_scored() {
    let input = SampleSpec::new(
      48000,
      2,
      PcmEncoding::I16,
      around_core::Interleave::Interleaved,
      around_core::ByteOrder::Little,
    )
    .unwrap();
    let output = SampleSpec::new(
      48000,
      2,
      PcmEncoding::I16,
      around_core::Interleave::Interleaved,
      around_core::ByteOrder::Big,
    )
    .unwrap();
    assert_eq!(conversion_loss(input, output).byte_order, 1);
  }
}
