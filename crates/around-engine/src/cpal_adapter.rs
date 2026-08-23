//! CPAL OutputDevice discovery and exact-format OutputBinding.
//!
//! This Adapter never resamples or changes PCM bytes. FilterChain has already
//! produced the exact native mode selected by OutputPlanner.

use crate::output::{create_output, AudioProducer};
use crate::output_plan::{
  DeviceSnapshot, OutputCapabilities, OutputCategory, OutputDevice, OutputMode, OutputPlan,
  SampleRateRange,
};
use crate::output_session::{
  BindingStatus, OutputBinder, OutputBinding, OutputDiscovery, OutputError,
};
use around_core::{AudioBufferRef, PcmEncoding, SampleSpec};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CpalOutputAdapter {
  priority: i16,
  revision: Arc<AtomicU64>,
}
impl Default for CpalOutputAdapter {
  fn default() -> Self {
    Self::new(0)
  }
}

impl CpalOutputAdapter {
  pub fn new(priority: i16) -> Self {
    Self {
      priority,
      revision: Arc::new(AtomicU64::new(0)),
    }
  }

  fn host(&self) -> cpal::Host {
    cpal::default_host()
  }

  fn device_id(name: &str, modes: &[OutputMode]) -> String {
    let mut fingerprint = name.to_owned();
    for mode in modes {
      fingerprint.push_str(&format!(
        ":{}:{}:{}-{}",
        mode.channels,
        mode.encoding.bits(),
        mode.rates.min,
        mode.rates.max
      ));
    }
    format!("cpal:{:016x}", stable_hash(fingerprint.as_bytes()))
  }

  fn enumerate_device(
    &self,
    device: &cpal::Device,
  ) -> Result<(OutputDevice, Vec<cpal::SupportedStreamConfigRange>), OutputError> {
    let name = device
      .name()
      .map_err(|error| OutputError::Discovery(error.to_string()))?;
    let ranges: Vec<_> = device
      .supported_output_configs()
      .map_err(|error| OutputError::Discovery(error.to_string()))?
      .collect();
    let default = device
      .default_output_config()
      .map_err(|error| OutputError::Discovery(error.to_string()))?;
    let mut modes = Vec::with_capacity(ranges.len());
    for (id, range) in ranges.iter().enumerate() {
      let Some(encoding) = map_sample_format(range.sample_format()) else {
        continue;
      };
      let min = range.min_sample_rate().0;
      let max = range.max_sample_rate().0;
      let mode = OutputMode {
        id: id as u32,
        channels: range.channels(),
        encoding,
        rates: SampleRateRange { min, max },
        default_rate: default.sample_rate().0.clamp(min, max),
        is_default: range.channels() == default.channels()
          && range.sample_format() == default.sample_format(),
        quality_rank: encoding.bits(),
      };
      modes.push(mode);
    }
    if modes.is_empty() {
      return Err(OutputError::Discovery(format!(
        "device {name} exposes no supported PCM mode"
      )));
    }
    let id = Self::device_id(&name, &modes);
    Ok((
      OutputDevice {
        id,
        name,
        category: OutputCategory::Other,
        priority: self.priority,
        capabilities: OutputCapabilities { modes },
      },
      ranges,
    ))
  }
}

impl OutputDiscovery for CpalOutputAdapter {
  fn snapshot(&self) -> Result<DeviceSnapshot, OutputError> {
    let host = self.host();
    let devices = host
      .output_devices()
      .map_err(|error| OutputError::Discovery(error.to_string()))?;
    let mut output = Vec::new();
    for device in devices {
      if let Ok((device, _)) = self.enumerate_device(&device) {
        output.push(device);
      }
    }
    output.sort_by(|left, right| left.id.cmp(&right.id));
    let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
    Ok(DeviceSnapshot {
      revision,
      devices: output,
    })
  }
}

impl OutputBinder for CpalOutputAdapter {
  fn bind(&self, plan: &OutputPlan) -> Result<Box<dyn OutputBinding>, OutputError> {
    if self.revision.load(Ordering::SeqCst) != plan.snapshot_revision {
      return Err(OutputError::Binding(
        "output capabilities changed since planning".into(),
      ));
    }
    let host = self.host();
    let devices = host
      .output_devices()
      .map_err(|error| OutputError::Discovery(error.to_string()))?;
    for device in devices {
      let Ok((description, ranges)) = self.enumerate_device(&device) else {
        continue;
      };
      if description.id != plan.device_id {
        continue;
      }
      let mode = description
        .capabilities
        .modes
        .iter()
        .find(|mode| mode.id == plan.mode_id)
        .ok_or_else(|| OutputError::Binding("planned mode disappeared".into()))?;
      let range = ranges
        .get(mode.id as usize)
        .ok_or_else(|| OutputError::Binding("planned CPAL range disappeared".into()))?;
      let Some(encoding) = map_sample_format(range.sample_format()) else {
        return Err(OutputError::Binding(
          "planned CPAL format is not representable".into(),
        ));
      };
      if encoding != plan.target_spec.encoding
        || range.channels() != u16::from(plan.target_spec.channels)
      {
        return Err(OutputError::Binding(
          "planned output encoding or channel count is stale".into(),
        ));
      }
      if !range.min_sample_rate().0.le(&plan.target_spec.sample_rate)
        || !range.max_sample_rate().0.ge(&plan.target_spec.sample_rate)
      {
        return Err(OutputError::Binding("planned sample rate is stale".into()));
      }
      let sample_format = range.sample_format();
      let config = cpal::StreamConfig {
        channels: plan.target_spec.channels as u16,
        sample_rate: cpal::SampleRate(plan.target_spec.sample_rate),
        buffer_size: cpal::BufferSize::Default,
      };
      let capacity = plan
        .target_spec
        .bytes_per_frame()
        .checked_mul(16384)
        .ok_or_else(|| OutputError::Binding("output ring size overflow".into()))?;
      let (producer, consumer) = create_output(capacity);
      let consumer = Arc::new(Mutex::new(consumer));
      let callback_consumer = Arc::clone(&consumer);
      let lost = Arc::new(AtomicBool::new(false));
      let callback_lost = Arc::clone(&lost);
      let stream = device
        .build_output_stream_raw(
          &config,
          sample_format,
          move |data, _| {
            let mut consumer = callback_consumer.lock();
            let bytes = data.bytes_mut();
            let popped = consumer.pop_slice(bytes);
            if popped < bytes.len() {
              fill_silence(encoding, &mut bytes[popped..]);
            }
          },
          move |error| {
            tracing::error!(?error, "cpal output stream failed");
            callback_lost.store(true, Ordering::SeqCst);
          },
          None,
        )
        .map_err(|error| OutputError::Binding(format!("failed to build CPAL stream: {error}")))?;
      stream
        .play()
        .map_err(|error| OutputError::Binding(format!("failed to start CPAL stream: {error}")))?;
      return Ok(Box::new(CpalOutputBinding {
        device_id: plan.device_id.clone(),
        spec: plan.target_spec,
        producer,
        stream: Some(stream),
        lost,
        closed: false,
      }));
    }
    Err(OutputError::Binding(format!(
      "output device {} is no longer available",
      plan.device_id
    )))
  }
}

struct CpalOutputBinding {
  device_id: String,
  spec: SampleSpec,
  producer: AudioProducer,
  stream: Option<cpal::Stream>,
  lost: Arc<AtomicBool>,
  closed: bool,
}

impl OutputBinding for CpalOutputBinding {
  fn device_id(&self) -> &str {
    &self.device_id
  }

  fn spec(&self) -> SampleSpec {
    self.spec
  }

  fn status(&self) -> BindingStatus {
    if self.closed {
      BindingStatus::Closed
    } else if self.lost.load(Ordering::SeqCst) {
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
    if block.spec() != self.spec || block.data().len() % self.spec.bytes_per_frame() != 0 {
      return Err(OutputError::Write(
        "output block does not match binding".into(),
      ));
    }
    let bytes = block.data();
    let mut offset = 0;
    while offset < bytes.len() {
      let pushed = self.producer.push_slice(&bytes[offset..]);
      offset += pushed;
      if pushed == 0 {
        if self.lost.load(Ordering::SeqCst) {
          return Err(OutputError::Lost);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
      }
    }
    Ok(())
  }

  fn close(&mut self) -> Result<(), OutputError> {
    if self.closed {
      return Ok(());
    }
    self.closed = true;
    if let Some(stream) = self.stream.take() {
      stream
        .pause()
        .map_err(|error| OutputError::Binding(format!("failed to stop CPAL stream: {error}")))?;
    }
    Ok(())
  }
}

unsafe impl Send for CpalOutputBinding {}

fn map_sample_format(format: cpal::SampleFormat) -> Option<PcmEncoding> {
  match format {
    cpal::SampleFormat::I8 => Some(PcmEncoding::I8),
    cpal::SampleFormat::I16 => Some(PcmEncoding::I16),
    cpal::SampleFormat::I32 => Some(PcmEncoding::I32),
    cpal::SampleFormat::I64 => Some(PcmEncoding::I64),
    cpal::SampleFormat::U8 => Some(PcmEncoding::U8),
    cpal::SampleFormat::U16 => Some(PcmEncoding::U16),
    cpal::SampleFormat::U32 => Some(PcmEncoding::U32),
    cpal::SampleFormat::U64 => Some(PcmEncoding::U64),
    cpal::SampleFormat::F32 => Some(PcmEncoding::F32),
    cpal::SampleFormat::F64 => Some(PcmEncoding::F64),
    _ => None,
  }
}

fn fill_silence(encoding: PcmEncoding, bytes: &mut [u8]) {
  let width = encoding.bytes_per_sample();
  if width == 0 {
    bytes.fill(0);
    return;
  }
  for sample in bytes.chunks_exact_mut(width) {
    let midpoint = if encoding.is_unsigned() {
      1_u128 << (u32::from(encoding.bits()) - 1)
    } else {
      0
    };
    let raw = midpoint.to_ne_bytes();
    sample.copy_from_slice(&raw[..width]);
  }
  let remainder = bytes.len() % width;
  if remainder != 0 {
    let start = bytes.len() - remainder;
    bytes[start..].fill(0);
  }
}

fn stable_hash(bytes: &[u8]) -> u64 {
  let mut hash = 0xcbf29ce484222325;
  for byte in bytes {
    hash ^= u64::from(*byte);
    hash = hash.wrapping_mul(0x100000001b3);
  }
  hash
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn maps_all_formats_exposed_by_cpal() {
    let formats = [
      (cpal::SampleFormat::I8, PcmEncoding::I8),
      (cpal::SampleFormat::I16, PcmEncoding::I16),
      (cpal::SampleFormat::I32, PcmEncoding::I32),
      (cpal::SampleFormat::I64, PcmEncoding::I64),
      (cpal::SampleFormat::U8, PcmEncoding::U8),
      (cpal::SampleFormat::U16, PcmEncoding::U16),
      (cpal::SampleFormat::U32, PcmEncoding::U32),
      (cpal::SampleFormat::U64, PcmEncoding::U64),
      (cpal::SampleFormat::F32, PcmEncoding::F32),
      (cpal::SampleFormat::F64, PcmEncoding::F64),
    ];
    for (format, expected) in formats {
      assert_eq!(map_sample_format(format), Some(expected));
    }
  }

  #[test]
  fn unsigned_silence_uses_native_midpoint() {
    let mut bytes = [0_u8; 4];
    fill_silence(PcmEncoding::U16, &mut bytes);
    assert_eq!(
      bytes,
      if cfg!(target_endian = "little") {
        [0, 128, 0, 128]
      } else {
        [128, 0, 128, 0]
      }
    );
  }
}
