//! Format-aware FilterChain (ADR-0006).
//!
//! Codec output is normalized f32 PCM. FilterChain owns every conversion from
//! that source format to the exact native `OutputBinding` format, including
//! sample rate, channel count, interleave, byte order, and encoding. Output
//! adapters only queue already-encoded bytes.

use crate::pcm_buffer::PcmBuffer;
use around_audio_sdk::filter::{
  AudioBufferC, AudioFormatC, AudioFormatListC, DynFilterRef, FilterDyn, FilterDynMut,
};
use around_core::{AudioBufferError, PcmEncoding, SampleSpec};
use std::fmt;

/// Score conversion loss according to ADR-0006.
pub fn score_format_pair(input: &SampleSpec, output: &SampleSpec) -> u32 {
  crate::output_plan::conversion_loss(*input, *output).total()
}
const MAX_CHANNELS: usize = 32;

/// Internal in-process Filter seam. Built-in Filters operate on normalized
/// interleaved f32; the terminal conversion is handled by FilterChain itself.
pub trait Filter: Send + Sync {
  fn process(&mut self, buf: &mut [f32], channels: u8) -> usize;
  fn info(&self) -> FilterInfo;
  fn channels_in(&self) -> Option<u8> {
    None
  }
  fn channels_out(&self) -> Option<u8> {
    None
  }
}

#[derive(Debug, Clone)]
pub struct FilterInfo {
  pub name: String,
  pub formats_in: Vec<SampleSpec>,
  pub formats_out: Vec<SampleSpec>,
}

/// Adapter from the format-aware stabby Filter ABI to the in-process Filter
/// seam. Dynamic Filters that declare non-f32 formats are rejected by this
/// narrow internal adapter until a typed Filter implementation is supplied.
pub struct StabbyFilterBridge {
  inner: DynFilterRef,
}

impl StabbyFilterBridge {
  pub fn new(inner: DynFilterRef) -> Self {
    Self { inner }
  }

  fn format_list(list: AudioFormatListC) -> Vec<SampleSpec> {
    list
      .as_slice()
      .iter()
      .filter_map(|format| {
        let encoding = match format.encoding {
          0 => PcmEncoding::I8,
          1 => PcmEncoding::I16,
          2 => PcmEncoding::I24,
          3 => PcmEncoding::I32,
          4 => PcmEncoding::I48,
          5 => PcmEncoding::I64,
          6 => PcmEncoding::U8,
          7 => PcmEncoding::U16,
          8 => PcmEncoding::U24,
          9 => PcmEncoding::U32,
          10 => PcmEncoding::U48,
          11 => PcmEncoding::U64,
          12 => PcmEncoding::F32,
          13 => PcmEncoding::F64,
          _ => return None,
        };
        let interleave = match format.interleave {
          0 => around_core::Interleave::Interleaved,
          1 => around_core::Interleave::Planar,
          _ => return None,
        };
        let byte_order = match format.byte_order {
          0 => around_core::ByteOrder::Native,
          1 => around_core::ByteOrder::Little,
          2 => around_core::ByteOrder::Big,
          _ => return None,
        };
        SampleSpec::new(
          format.sample_rate,
          format.channels,
          encoding,
          interleave,
          byte_order,
        )
        .ok()
      })
      .collect()
  }
}

impl Filter for StabbyFilterBridge {
  fn process(&mut self, buf: &mut [f32], channels: u8) -> usize {
    let format = AudioFormatC {
      sample_rate: 0,
      channels,
      encoding: 12,
      interleave: 0,
      byte_order: 0,
    };
    let mut cbuf = AudioBufferC {
      input_data: buf.as_ptr().cast(),
      input_len: std::mem::size_of_val(buf),
      output_data: buf.as_mut_ptr().cast(),
      output_len: std::mem::size_of_val(buf),
      frames: buf.len() / usize::from(channels.max(1)),
      input_format: format,
      output_format: format,
    };
    let frames = self.inner.process(&mut cbuf) as usize;
    frames
      .saturating_mul(usize::from(channels.max(1)))
      .min(buf.len())
  }

  fn info(&self) -> FilterInfo {
    let name_ptr = self.inner.name();
    let name = if name_ptr.is_null() {
      "unknown".into()
    } else {
      unsafe {
        std::ffi::CStr::from_ptr(name_ptr as *const i8)
          .to_string_lossy()
          .into_owned()
      }
    };
    FilterInfo {
      name,
      formats_in: Self::format_list(self.inner.formats_in()),
      formats_out: Self::format_list(self.inner.formats_out()),
    }
  }
}

unsafe impl Send for StabbyFilterBridge {}
unsafe impl Sync for StabbyFilterBridge {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
  InputFormat,
  OutputTooSmall { required: usize, available: usize },
  WorkspaceTooSmall { required: usize, available: usize },
  UnsupportedPlanarInput,
  Buffer(AudioBufferError),
}

impl fmt::Display for FilterError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::InputFormat => f.write_str("FilterChain received a non-f32 input"),
      Self::OutputTooSmall {
        required,
        available,
      } => {
        write!(
          f,
          "FilterChain output needs {required} bytes, has {available}"
        )
      }
      Self::WorkspaceTooSmall {
        required,
        available,
      } => {
        write!(
          f,
          "FilterChain workspace needs {required} samples, has {available}"
        )
      }
      Self::UnsupportedPlanarInput => f.write_str("planar codec input is not supported yet"),
      Self::Buffer(error) => error.fmt(f),
    }
  }
}

impl std::error::Error for FilterError {}

impl From<AudioBufferError> for FilterError {
  fn from(error: AudioBufferError) -> Self {
    Self::Buffer(error)
  }
}

pub struct FilterChain {
  filters: Vec<Box<dyn Filter>>,
  decode_spec: SampleSpec,
  output_spec: SampleSpec,
  resample_position: f64,
  previous_frame: [f32; MAX_CHANNELS],
  has_previous_frame: bool,
}
impl FilterChain {
  pub fn build(
    decode_spec: SampleSpec,
    filters: Vec<Box<dyn Filter>>,
    output_spec: SampleSpec,
  ) -> Self {
    Self {
      filters,
      decode_spec,
      output_spec,
      resample_position: 0.0,
      previous_frame: [0.0; MAX_CHANNELS],
      has_previous_frame: false,
    }
  }
  pub fn source_spec(&self) -> SampleSpec {
    self.decode_spec
  }

  pub fn output_spec(&self) -> SampleSpec {
    self.output_spec
  }

  pub fn reconfigure_output(&mut self, output_spec: SampleSpec) {
    self.output_spec = output_spec;
    self.resample_position = 0.0;
    self.previous_frame.fill(0.0);
    self.has_previous_frame = false;
  }

  pub fn workspace_requirements(&self, input_samples: usize) -> (usize, usize) {
    let input_frames = input_samples / usize::from(self.decode_spec.channels.max(1));
    let output_frames = ((input_frames as u128 * u128::from(self.output_spec.sample_rate)
      + u128::from(self.decode_spec.sample_rate - 1))
      / u128::from(self.decode_spec.sample_rate)) as usize
      + 2;
    let output_samples = output_frames * usize::from(self.output_spec.channels.max(1));
    let output_bytes = output_frames * self.output_spec.bytes_per_frame();
    (input_samples.max(output_samples), output_bytes)
  }

  pub fn is_empty(&self) -> bool {
    self.filters.is_empty() && self.decode_spec == self.output_spec
  }

  pub fn len(&self) -> usize {
    self.filters.len()
  }

  /// Process one decoded f32 block and encode it into the exact output format.
  /// All temporary storage comes from `workspace`; this method does not grow
  /// or allocate any collection.
  pub fn process_to_output(
    &mut self,
    input: &[f32],
    frames: usize,
    workspace: &mut PcmBuffer,
  ) -> Result<usize, FilterError> {
    if self.decode_spec.encoding != PcmEncoding::F32
      || self.decode_spec.interleave != around_core::Interleave::Interleaved
    {
      return Err(FilterError::InputFormat);
    }
    let channels = usize::from(self.decode_spec.channels);
    let expected = frames
      .checked_mul(channels)
      .ok_or(FilterError::InputFormat)?;
    if input.len() != expected {
      return Err(FilterError::InputFormat);
    }

    let (filter_buf, scratch, output) = workspace.processing_regions();
    if filter_buf.len() < input.len() {
      return Err(FilterError::WorkspaceTooSmall {
        required: input.len(),
        available: filter_buf.len(),
      });
    }
    filter_buf[..input.len()].copy_from_slice(input);
    let mut sample_count = input.len();
    let mut current_channels = self.decode_spec.channels;
    if usize::from(current_channels) > MAX_CHANNELS {
      return Err(FilterError::InputFormat);
    }
    for filter in &mut self.filters {
      sample_count = filter
        .process(&mut filter_buf[..sample_count], current_channels)
        .min(filter_buf.len());
      if let Some(channels_out) = filter.channels_out() {
        current_channels = channels_out;
        if usize::from(current_channels) > MAX_CHANNELS {
          return Err(FilterError::InputFormat);
        }
      }
    }

    let input_frames = sample_count / usize::from(current_channels.max(1));
    let target_channels = usize::from(self.output_spec.channels.max(1));
    let capacity_frames = scratch.len() / target_channels;
    let required_frames = if self.decode_spec.sample_rate == self.output_spec.sample_rate {
      input_frames
    } else {
      ((input_frames as u128 * u128::from(self.output_spec.sample_rate)
        + u128::from(self.decode_spec.sample_rate - 1))
        / u128::from(self.decode_spec.sample_rate)) as usize
        + 2
    };
    if capacity_frames < required_frames {
      return Err(FilterError::WorkspaceTooSmall {
        required: required_frames * target_channels,
        available: scratch.len(),
      });
    }
    let output_frames = if self.decode_spec.sample_rate == self.output_spec.sample_rate {
      convert_f32_stateless(
        &filter_buf[..sample_count],
        input_frames,
        current_channels,
        &mut scratch[..input_frames * target_channels],
        input_frames,
        self.output_spec,
      );
      input_frames
    } else {
      convert_f32_stateful(
        &filter_buf[..sample_count],
        input_frames,
        current_channels,
        &mut scratch[..capacity_frames * target_channels],
        capacity_frames,
        self.decode_spec.sample_rate,
        self.output_spec,
        &mut self.resample_position,
        &mut self.previous_frame,
        &mut self.has_previous_frame,
      )
    };
    encode_f32(
      &scratch[..output_frames * target_channels],
      output_frames,
      self.output_spec,
      output,
    )
  }

  pub fn process_workspace(
    &mut self,
    workspace: &mut PcmBuffer,
    frames: usize,
  ) -> Result<usize, FilterError> {
    let samples = frames
      .checked_mul(usize::from(self.decode_spec.channels))
      .ok_or(FilterError::InputFormat)?;
    let decode_capacity = workspace.decode_region().len();
    if samples > decode_capacity {
      return Err(FilterError::WorkspaceTooSmall {
        required: samples,
        available: decode_capacity,
      });
    }
    let input = workspace.decode_region().as_ptr();
    let input = unsafe { std::slice::from_raw_parts(input, samples) };
    self.process_to_output(input, frames, workspace)
  }
}

fn convert_f32_stateless(
  input: &[f32],
  input_frames: usize,
  input_channels: u8,
  output: &mut [f32],
  output_frames: usize,
  output_spec: SampleSpec,
) {
  let in_channels = usize::from(input_channels.max(1));
  let out_channels = usize::from(output_spec.channels.max(1));
  let ratio = (input_frames.max(1) as f64) / (output_frames.max(1) as f64);
  for out_frame in 0..output_frames {
    let source_pos = out_frame as f64 * ratio;
    let source_frame = source_pos.floor() as usize;
    let next_frame = (source_frame + 1).min(input_frames.saturating_sub(1));
    let fraction = (source_pos - source_frame as f64) as f32;
    write_mapped_frame(
      input,
      in_channels,
      source_frame,
      next_frame,
      fraction,
      output,
      out_channels,
      out_frame,
    );
  }
}
#[expect(
  clippy::too_many_arguments,
  reason = "the hot-path conversion kernel keeps buffer and format state explicit"
)]
fn write_mapped_frame(
  input: &[f32],
  input_channels: usize,
  source_frame: usize,
  next_frame: usize,
  fraction: f32,
  output: &mut [f32],
  output_channels: usize,
  output_frame: usize,
) {
  for output_channel in 0..output_channels {
    let value = if output_channels == 1 {
      let mut sum = 0.0;
      for channel in 0..input_channels {
        let a = input[source_frame * input_channels + channel];
        let b = input[next_frame * input_channels + channel];
        sum += a + (b - a) * fraction;
      }
      sum / input_channels as f32
    } else {
      let source_channel = if input_channels == 1 {
        0
      } else {
        output_channel.min(input_channels - 1)
      };
      let a = input[source_frame * input_channels + source_channel];
      let b = input[next_frame * input_channels + source_channel];
      a + (b - a) * fraction
    };
    output[output_frame * output_channels + output_channel] = value;
  }
}

#[expect(
  clippy::too_many_arguments,
  reason = "the hot-path conversion kernel keeps buffer and format state explicit"
)]
fn convert_f32_stateful(
  input: &[f32],
  input_frames: usize,
  input_channels: u8,
  output: &mut [f32],
  output_capacity_frames: usize,
  input_rate: u32,
  output_spec: SampleSpec,
  position: &mut f64,
  previous_frame: &mut [f32],
  has_previous_frame: &mut bool,
) -> usize {
  if input_frames == 0 {
    return 0;
  }
  let in_channels = usize::from(input_channels.max(1));
  if in_channels > previous_frame.len() {
    return 0;
  }
  let out_channels = usize::from(output_spec.channels.max(1));
  let ratio = f64::from(input_rate) / f64::from(output_spec.sample_rate);
  let sequence_offset = usize::from(*has_previous_frame);
  let sequence_len = input_frames + sequence_offset;
  let mut produced = 0;
  while *position + 1.0 < sequence_len as f64 && produced < output_capacity_frames {
    let source_pos = *position;
    let base = source_pos.floor() as isize;
    let next = base + 1;
    let fraction = (source_pos - base as f64) as f32;
    for out_channel in 0..out_channels {
      let value = mapped_stateful_sample(
        input,
        input_frames,
        in_channels,
        sequence_offset,
        previous_frame,
        base,
        next,
        fraction,
        out_channel,
        out_channels,
      );
      output[produced * out_channels + out_channel] = value;
    }
    *position += ratio;
    produced += 1;
  }
  let consumed = (*position).floor().max(0.0) as usize;
  let previous_index = consumed
    .saturating_sub(sequence_offset)
    .min(input_frames - 1);
  previous_frame[..in_channels]
    .copy_from_slice(&input[previous_index * in_channels..(previous_index + 1) * in_channels]);
  *has_previous_frame = true;
  *position -= (sequence_len.saturating_sub(1)) as f64;
  if *position < 0.0 {
    *position = 0.0;
  }
  produced
}

#[expect(
  clippy::too_many_arguments,
  reason = "the hot-path conversion kernel keeps buffer and format state explicit"
)]
fn mapped_stateful_sample(
  input: &[f32],
  input_frames: usize,
  in_channels: usize,
  sequence_offset: usize,
  previous_frame: &[f32],
  base: isize,
  next: isize,
  fraction: f32,
  out_channel: usize,
  out_channels: usize,
) -> f32 {
  let sample = |index: isize, channel: usize| {
    let source_channel = if in_channels == 1 {
      0
    } else {
      channel.min(in_channels - 1)
    };
    if sequence_offset == 1 && index == 0 {
      return previous_frame[source_channel];
    }
    let current = if sequence_offset == 1 {
      (index - 1).clamp(0, input_frames.saturating_sub(1) as isize) as usize
    } else {
      index.clamp(0, input_frames.saturating_sub(1) as isize) as usize
    };
    input[current * in_channels + source_channel]
  };
  if out_channels == 1 {
    let mut sum = 0.0;
    for channel in 0..in_channels {
      sum += sample(base, channel) + (sample(next, channel) - sample(base, channel)) * fraction;
    }
    sum / in_channels as f32
  } else {
    let channel = if in_channels == 1 {
      0
    } else {
      out_channel.min(in_channels - 1)
    };
    sample(base, channel) + (sample(next, channel) - sample(base, channel)) * fraction
  }
}

fn encode_f32(
  samples: &[f32],
  frames: usize,
  spec: SampleSpec,
  output: &mut [u8],
) -> Result<usize, FilterError> {
  let required = frames
    .checked_mul(spec.bytes_per_frame())
    .ok_or(FilterError::OutputTooSmall {
      required: usize::MAX,
      available: output.len(),
    })?;
  if output.len() < required {
    return Err(FilterError::OutputTooSmall {
      required,
      available: output.len(),
    });
  }
  let channels = usize::from(spec.channels);
  if spec.interleave == around_core::Interleave::Planar {
    for channel in 0..channels {
      for frame in 0..frames {
        write_sample(
          samples[frame * channels + channel],
          spec.encoding,
          spec.byte_order,
          &mut output[(channel * frames + frame) * spec.bytes_per_sample()..],
        );
      }
    }
  } else {
    for (index, sample) in samples.iter().copied().enumerate() {
      write_sample(
        sample,
        spec.encoding,
        spec.byte_order,
        &mut output[index * spec.bytes_per_sample()..],
      );
    }
  }
  Ok(required)
}

fn write_sample(value: f32, encoding: PcmEncoding, order: around_core::ByteOrder, out: &mut [u8]) {
  let value = if value.is_finite() {
    value.clamp(-1.0, 1.0)
  } else {
    0.0
  };
  let little = match order {
    around_core::ByteOrder::Little => true,
    around_core::ByteOrder::Big => false,
    around_core::ByteOrder::Native => cfg!(target_endian = "little"),
  };
  match encoding {
    PcmEncoding::F32 => copy_ordered(&value.to_ne_bytes(), out, little),
    PcmEncoding::F64 => copy_ordered(&(f64::from(value)).to_ne_bytes(), out, little),
    _ if encoding.is_signed() => {
      let bits = encoding.bits();
      let max = ((1_i128 << (bits - 1)) - 1) as f64;
      let min = -(1_i128 << (bits - 1)) as f64;
      let integer = (f64::from(value) * if value < 0.0 { -min } else { max })
        .round()
        .clamp(min, max) as i128;
      write_integer(integer as u128, encoding.bytes_per_sample(), little, out);
    }
    _ => {
      let bits = encoding.bits();
      let max = ((1_u128 << bits) - 1) as f64;
      let integer = ((f64::from(value) + 1.0) * 0.5 * max)
        .round()
        .clamp(0.0, max) as u128;
      write_integer(integer, encoding.bytes_per_sample(), little, out);
    }
  }
}

fn write_integer(value: u128, width: usize, little: bool, out: &mut [u8]) {
  let bytes = value.to_le_bytes();
  if little {
    out[..width].copy_from_slice(&bytes[..width]);
  } else {
    for (dst, src) in out[..width].iter_mut().zip(bytes[..width].iter().rev()) {
      *dst = *src;
    }
  }
}

fn copy_ordered(native: &[u8], out: &mut [u8], little: bool) {
  if little == cfg!(target_endian = "little") {
    out[..native.len()].copy_from_slice(native);
  } else {
    for (dst, src) in out[..native.len()].iter_mut().zip(native.iter().rev()) {
      *dst = *src;
    }
  }
}

pub struct Volume {
  gain: f32,
}

impl Volume {
  pub fn new(gain_db: f32) -> Self {
    Self {
      gain: 10.0f32.powf(gain_db / 20.0),
    }
  }

  pub fn with_gain(gain: f32) -> Self {
    Self { gain }
  }
}

impl Filter for Volume {
  fn process(&mut self, buf: &mut [f32], _channels: u8) -> usize {
    for sample in buf.iter_mut() {
      *sample *= self.gain;
    }
    buf.len()
  }

  fn info(&self) -> FilterInfo {
    FilterInfo {
      name: format!("volume({:.1}dB)", 20.0 * self.gain.log10()),
      formats_in: vec![],
      formats_out: vec![],
    }
  }
}

#[cfg(test)]
mod tests {
  #![expect(
    clippy::unwrap_used,
    reason = "test fixtures use validated formats and fixed buffers"
  )]
  use super::*;
  use around_audio_sdk::filter::{AudioBufferC, AudioFormatListC};

  struct AbiGain;

  impl around_audio_sdk::filter::Filter for AbiGain {
    extern "C" fn name(&self) -> *const u8 {
      c"abi-gain".as_ptr().cast()
    }
    extern "C" fn formats_in(&self) -> AudioFormatListC {
      AudioFormatListC::any()
    }
    extern "C" fn formats_out(&self) -> AudioFormatListC {
      AudioFormatListC::any()
    }
    extern "C" fn process(&mut self, buf: &mut AudioBufferC) -> u32 {
      let input =
        unsafe { std::slice::from_raw_parts(buf.input_data.cast::<f32>(), buf.input_len / 4) };
      let output = unsafe {
        std::slice::from_raw_parts_mut(buf.output_data.cast::<f32>(), buf.output_len / 4)
      };
      let len = input.len().min(output.len());
      for (dst, src) in output[..len].iter_mut().zip(&input[..len]) {
        *dst = *src * 0.5;
      }
      u32::try_from(len / usize::from(buf.output_format.channels.max(1))).unwrap_or(u32::MAX)
    }
    extern "C" fn reset(&mut self) {}
  }

  fn f32_spec() -> SampleSpec {
    SampleSpec::interleaved(44100, 2, PcmEncoding::F32).unwrap()
  }

  #[test]
  fn empty_chain_passthrough() {
    let spec = f32_spec();
    let chain = FilterChain::build(spec, vec![], spec);
    assert!(chain.is_empty());
  }

  #[test]
  fn stateful_rate_conversion_preserves_duration_across_chunks() {
    let source = SampleSpec::interleaved(44100, 1, PcmEncoding::F32).unwrap();
    let target = SampleSpec::interleaved(48000, 1, PcmEncoding::F32).unwrap();
    let mut chain = FilterChain::build(source, vec![], target);
    let input = vec![0.0_f32; 44100];
    let mut written_frames = 0;
    for chunk in input.chunks(997) {
      let (processing, output_bytes) = chain.workspace_requirements(chunk.len());
      let mut workspace =
        PcmBuffer::with_processing_capacity(chunk.len(), processing, output_bytes);
      written_frames += chain
        .process_to_output(chunk, chunk.len(), &mut workspace)
        .unwrap()
        / target.bytes_per_frame();
    }
    assert!((47_900..=48_100).contains(&written_frames));
  }

  #[test]
  fn stateful_rate_conversion_keeps_ramp_ordered_across_chunks() {
    let source = SampleSpec::interleaved(44100, 1, PcmEncoding::F32).unwrap();
    let target = SampleSpec::interleaved(48000, 1, PcmEncoding::F32).unwrap();
    let mut chain = FilterChain::build(source, vec![], target);
    let input: Vec<f32> = (0..441).map(|sample| sample as f32).collect();
    let mut output = Vec::new();
    for chunk in input.chunks(73) {
      let (processing, output_bytes) = chain.workspace_requirements(chunk.len());
      let mut workspace =
        PcmBuffer::with_processing_capacity(chunk.len(), processing, output_bytes);
      let written = chain
        .process_to_output(chunk, chunk.len(), &mut workspace)
        .unwrap();
      let samples = unsafe {
        std::slice::from_raw_parts(
          workspace.output_region().as_ptr().cast::<f32>(),
          written / 4,
        )
      };
      output.extend_from_slice(samples);
    }
    assert!(output.windows(2).all(|pair| pair[0] <= pair[1]));
  }

  #[test]
  fn volume_attenuates() {
    let mut vol = Volume::with_gain(0.5);
    let mut buf = vec![1.0f32; 10];
    vol.process(&mut buf, 1);
    assert!((buf[0] - 0.5).abs() < 0.001);
  }

  #[test]
  fn terminal_conversion_changes_channels_and_encoding() {
    let source = f32_spec();
    let target = SampleSpec::interleaved(44100, 1, PcmEncoding::I16).unwrap();
    let mut chain = FilterChain::build(source, vec![], target);
    let mut workspace = PcmBuffer::new(8, 128);
    let input = [1.0_f32, 1.0, 0.5, 0.5];
    let written = chain.process_to_output(&input, 2, &mut workspace).unwrap();
    assert_eq!(written, 4);
    assert_eq!(workspace.output_region()[0..2], i16::MAX.to_ne_bytes());
  }

  #[test]
  fn stabby_filter_bridge_processes_sdk_filter() {
    let spec = f32_spec();
    let bridge = StabbyFilterBridge::new(around_audio_sdk::filter::make_dyn_filter(AbiGain));
    let mut chain = FilterChain::build(spec, vec![Box::new(bridge)], spec);
    let mut workspace = PcmBuffer::new(8, 128);
    let input = [1.0_f32, -1.0, 0.5, -0.5];
    let written = chain.process_to_output(&input, 2, &mut workspace).unwrap();
    assert_eq!(written, std::mem::size_of_val(&input));
    let samples = unsafe {
      std::slice::from_raw_parts(
        workspace.output_region().as_ptr().cast::<f32>(),
        input.len(),
      )
    };
    assert_eq!(samples, &[0.5, -0.5, 0.25, -0.25]);
  }

  #[test]
  fn scoring_covers_rate_channels_interleave_and_encoding() {
    let input = f32_spec();
    let output = SampleSpec::new(
      48000,
      1,
      PcmEncoding::I16,
      around_core::Interleave::Planar,
      around_core::ByteOrder::Native,
    )
    .unwrap();
    assert!(score_format_pair(&input, &output) > 0);
  }

  #[test]
  fn terminal_encoder_covers_all_declared_pcm_widths() {
    let encodings = [
      PcmEncoding::I8,
      PcmEncoding::I16,
      PcmEncoding::I24,
      PcmEncoding::I32,
      PcmEncoding::I48,
      PcmEncoding::I64,
      PcmEncoding::U8,
      PcmEncoding::U16,
      PcmEncoding::U24,
      PcmEncoding::U32,
      PcmEncoding::U48,
      PcmEncoding::U64,
      PcmEncoding::F32,
      PcmEncoding::F64,
    ];
    let source = SampleSpec::interleaved(44100, 1, PcmEncoding::F32).unwrap();
    for encoding in encodings {
      let target = SampleSpec::interleaved(44100, 1, encoding).unwrap();
      let mut chain = FilterChain::build(source, vec![], target);
      let mut workspace = PcmBuffer::new(3, 3 * target.bytes_per_frame());
      let written = chain
        .process_to_output(&[-1.0, 0.0, 1.0], 3, &mut workspace)
        .unwrap();
      assert_eq!(written, 3 * target.bytes_per_frame(), "{encoding:?}");
    }
  }

  #[test]
  fn terminal_encoder_preserves_planar_layout() {
    let source = f32_spec();
    let target = SampleSpec::new(
      44100,
      2,
      PcmEncoding::I16,
      around_core::Interleave::Planar,
      around_core::ByteOrder::Native,
    )
    .unwrap();
    let mut chain = FilterChain::build(source, vec![], target);
    let mut workspace = PcmBuffer::new(4, 16);
    let written = chain
      .process_to_output(&[1.0, 0.5, -1.0, -0.5], 2, &mut workspace)
      .unwrap();
    assert_eq!(written, 8);
  }
}
