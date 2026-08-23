//! Format-aware Filter trait (stabby ABI), register, and helpers.
//!
//! The ABI intentionally passes explicit input and output buffers. A Filter may
//! preserve a format or convert it; the host never assumes interleaved f32.

use stabby::boxed::Box;

/// Stable encoding identifiers used by the Filter ABI.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEncodingC {
  I8 = 0,
  I16 = 1,
  I24 = 2,
  I32 = 3,
  I48 = 4,
  I64 = 5,
  U8 = 6,
  U16 = 7,
  U24 = 8,
  U32 = 9,
  U48 = 10,
  U64 = 11,
  F32 = 12,
  F64 = 13,
}

/// Stable PCM format descriptor crossing the extension Seam.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormatC {
  pub sample_rate: u32,
  pub channels: u8,
  pub encoding: u8,
  pub interleave: u8,
  pub byte_order: u8,
}

impl AudioFormatC {
  pub const fn any() -> Self {
    Self {
      sample_rate: 0,
      channels: 0,
      encoding: u8::MAX,
      interleave: u8::MAX,
      byte_order: u8::MAX,
    }
  }
}

/// Fixed-capacity format declaration. `len == 0` means any format.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioFormatListC {
  pub formats: [AudioFormatC; 4],
  pub len: u8,
}

impl AudioFormatListC {
  pub const fn any() -> Self {
    Self {
      formats: [AudioFormatC::any(); 4],
      len: 0,
    }
  }

  pub fn as_slice(&self) -> &[AudioFormatC] {
    let len = usize::from(self.len.min(4));
    &self.formats[..len]
  }
}

/// Explicit input/output byte buffers passed to a Filter.
///
/// # Safety
///
/// `input_data` must point to `input_len` readable bytes and `output_data` to
/// `output_len` writable bytes for the duration of `process`. The host owns
/// both allocations and does not retain them after the call.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioBufferC {
  pub input_data: *const u8,
  pub input_len: usize,
  pub output_data: *mut u8,
  pub output_len: usize,
  pub frames: usize,
  pub input_format: AudioFormatC,
  pub output_format: AudioFormatC,
}

#[stabby::stabby]
#[around_extensions_macros::slot]
pub trait Filter: Send + Sync {
  extern "C" fn name(&self) -> *const u8;

  /// Declare accepted input formats. An empty list means any format.
  extern "C" fn formats_in(&self) -> AudioFormatListC;

  /// Declare produced output formats. An empty list means same as input.
  extern "C" fn formats_out(&self) -> AudioFormatListC;

  /// Process `frames` frames from input into output and return produced frames.
  extern "C" fn process(&mut self, buf: &mut AudioBufferC) -> u32;

  extern "C" fn reset(&mut self);
}

pub fn make_dyn_filter(filter: impl Filter + 'static) -> DynFilterRef {
  let bx = stabby::boxed::Box::new(filter);
  DynFilterRef::from(bx)
}

pub fn init_filter() -> &'static FilterRegister {
  use std::sync::LazyLock;
  static REG: LazyLock<FilterRegister> = LazyLock::new(FilterRegister::new);
  let reg: &FilterRegister = &REG;
  let reg_ptr: *const FilterRegister = reg;
  around_extensions::Framework::instance().attach_register(
    NAME,
    &REGISTER_VTABLE,
    reg_ptr as *mut (),
  );
  reg
}
