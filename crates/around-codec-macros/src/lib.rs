//! Proc macro for codec boilerplate generation.
//!
//! `#[register_codec]` generates:
//! - `AudioStreamVTable` static with fn pointer wrappers
//! - `CodecInfo` static with format claims
//!
//! The engine still calls `registry.push(info)` explicitly — this macro
//! is pure codegen, not magic registration.
//!
//! ## Usage
//!
//! ```ignore
//! #[register_codec(
//!     name = "wav",
//!     extensions = ["wav"],
//!     magics = ["RIFF"],
//!     mime_types = ["audio/wav"],
//! )]
//! impl WavCodec {
//!     fn open(reader: Box<dyn ReadSeek + Send + Sync>) -> Result<AudioStream, AroundError> { .. }
//!     fn read(data: *mut c_void, buf: &mut [f32]) -> Result<Option<usize>, AroundError> { .. }
//!     fn seek(data: *mut c_void, frame: u64) -> Result<(), AroundError> { .. }
//!     fn drop(data: *mut c_void) { .. }
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemImpl};

#[proc_macro_attribute]
pub fn register_codec(attr: TokenStream, item: TokenStream) -> TokenStream {
  let _attr = proc_macro2::TokenStream::from(attr);
  let item_impl = parse_macro_input!(item as ItemImpl);

  // Stub: generates the input unchanged. Future: generate vtable + CodecInfo.
  let expanded = quote! {
      #item_impl
  };

  TokenStream::from(expanded)
}
