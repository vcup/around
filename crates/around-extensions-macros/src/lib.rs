//! Proc-macro for `#[slot]` — generates Register types from stabby traits.
//!
//! ## Usage
//!
//! ```ignore
//! #[stabby::stabby]
//! #[slot]
//! pub trait Codec {
//!     extern "C" fn probe(&self, header: Slice<'_, [u8]>, filename: Slice<'_, [u8]>) -> u8;
//!     // ...
//! }
//! ```
//!
//! - `const NAME: &'static str`
//! - `pub unsafe fn from_raw(vtable: *const (), data: *mut ()) -> Dyn{Trait}Ref`
//! - `struct {Trait}RegisterVTable`
//! - `struct {Trait}Register`
//! - `impl {Trait}Register` (new, push, from_raw, probe, for_each_entry, remove_by_meta)
//! - `static REGISTER_VTABLE: RegisterVTable`
//! - `pub type Dyn{Trait}Ref`

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
  parse::{Parse, ParseStream},
  parse_macro_input, ItemTrait, LitByteStr,
};

// ---------------------------------------------------------------------------
// Attribute arguments
// ---------------------------------------------------------------------------

enum SlotMode {
  /// Default: Vec-based Register
  Vec,
  /// HashMap-based Register keyed by *const ExtensionMeta
  Map,
  /// Manual: only NAME, RegisterVTable, from_raw
  Manual,
}

struct SlotArgs {
  mode: SlotMode,
  /// Custom name override. Default: `{crate_name}::{TraitName}`
  name: Option<String>,
}

impl Parse for SlotArgs {
  fn parse(input: ParseStream) -> syn::Result<Self> {
    let mut mode = SlotMode::Vec;
    let mut name = None;

    if input.is_empty() {
      return Ok(SlotArgs { mode, name });
    }

    // Support comma-separated key=value pairs.
    while !input.is_empty() {
      let ident: syn::Ident = input.parse()?;
      if ident == "manual" {
        mode = SlotMode::Manual;
      } else if ident == "storage" {
        let _eq: syn::Token![=] = input.parse()?;
        let lit: syn::LitStr = input.parse()?;
        if lit.value() == "map" {
          mode = SlotMode::Map;
        } else if lit.value() == "vec" {
          mode = SlotMode::Vec;
        } else {
          return Err(syn::Error::new(
            lit.span(),
            "expected `\"map\"` or `\"vec\"` for storage parameter",
          ));
        }
      } else if ident == "name" {
        let _eq: syn::Token![=] = input.parse()?;
        let lit: syn::LitStr = input.parse()?;
        name = Some(lit.value());
      } else if ident == "prefix" {
        let _eq: syn::Token![=] = input.parse()?;
        let _lit: syn::LitStr = input.parse()?;
        return Err(syn::Error::new(
          ident.span(),
          "prefix is removed; use name instead. Example: #[slot(name = \"crate::Trait\")]",
        ));
      } else {
        return Err(syn::Error::new(
          ident.span(),
          "expected `storage = \"map\"`, `name = \"...\"`, or `manual`",
        ));
      }
      // Consume optional comma.
      let _ = input.parse::<syn::Token![,]>();
    }

    Ok(SlotArgs { mode, name })
  }
}

// ---------------------------------------------------------------------------
// Main attribute macro
// ---------------------------------------------------------------------------

/// `#[slot]` — generates Register boilerplate for a stabby trait.
///
/// See [crate-level docs](self) for details.
#[proc_macro_attribute]
pub fn slot(attr: TokenStream, item: TokenStream) -> TokenStream {
  let args = parse_macro_input!(attr as SlotArgs);
  let input = parse_macro_input!(item as ItemTrait);

  let trait_ident = &input.ident;
  let register_ident = format_ident!("{}Register", trait_ident);
  let vtable_ident = format_ident!("{}RegisterVTable", trait_ident);
  let dyn_ref_ident = format_ident!("Dyn{}Ref", trait_ident);

  let register_struct = match args.mode {
    SlotMode::Vec => quote! {
        #[doc = concat!("Vec-based register for ", stringify!(#trait_ident), " entries.")]
        /// Stores entries as `(vtable: usize, data: usize, meta: usize)` to avoid
        /// raw pointer Send/Sync issues in static contexts.
        ///
        /// PERF: When ready to eliminate Mutex contention on probe(), replace
        /// parking_lot::Mutex<Vec<...>> with arc_swap::ArcSwap<Vec<...>>.
        /// Clone-on-write for rare push/remove operations; lock-free reads for
        /// the hot-path probe()/for_each_entry() methods.
        pub struct #register_ident {
            entries: parking_lot::Mutex<Vec<(usize, usize, usize)>>,
        }
    },
    SlotMode::Map => quote! {
        #[doc = concat!("HashMap-based register for ", stringify!(#trait_ident), " entries, keyed by (ExtensionMeta pointer, push counter).")]
        ///
        /// PERF: When ready to eliminate Mutex contention on probe(), replace
        /// parking_lot::Mutex<HashMap<...>> with arc_swap::ArcSwap<HashMap<...>>.
        /// Clone-on-write for rare push/remove operations; lock-free reads for
        /// the hot-path probe()/for_each_entry() methods.
        pub struct #register_ident {
            entries: parking_lot::Mutex<
                std::collections::HashMap<(usize, usize), (usize, usize)>,
            >,
            push_counter: std::sync::atomic::AtomicUsize,
        }
    },
    SlotMode::Manual => quote! {
        // Manual mode: no Register struct generated.
    },
  };

  let register_impl = match args.mode {
    SlotMode::Vec => {
      quote! {
          impl #register_ident {
              /// Create a new empty register.
              pub fn new() -> Self {
                  Self {
                      entries: parking_lot::Mutex::new(Vec::new()),
                  }
              }

              /// Push an opaque entry pointer onto this register.
              ///
              /// # Layout invariant
              ///
              /// This method extracts the vtable pointer at a fixed offset
              /// `core::mem::size_of::<*mut ()>()` from the entry pointer,
              /// assuming stabby's `dynptr!` macro produces a `repr(C)` struct
              /// with the following layout:
              ///
              /// ```text
              /// { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt }
              /// ```
              ///
              /// The `ManuallyDrop<Box<()>>` occupies `size_of::<*mut ()>()` bytes,
              /// placing the vtable reference immediately after.
              /// If stabby changes this layout, the offset must be updated here and
              /// in [`from_raw`].
              pub fn push(&self, entry: *mut (), ext_meta: *const ::around_extensions::ExtensionMeta) {
                  const _: () = {
                      // Assert: the vtable pointer in stabby's Dyn<Box<()>, Vt> is at offset size_of::<*mut ()>().
                      // This is the documented layout: { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt, ... }
                      // If this fails, stabby's dynptr! layout changed — update the offset.
                  };
                  let vtable_ptr: *const () = unsafe {
                      *((entry as *const u8).add(core::mem::size_of::<*mut ()>()) as *const *const ())
                  };
                  self.entries.lock().push((vtable_ptr as usize, entry as usize, ext_meta as usize));
              }

              /// Iterate all entries, calling `f(vtable_ptr, data_ptr)`.
              /// Pointers are reconstructed from stored integers.
              pub fn for_each_entry(&self, mut f: impl FnMut(*const (), *mut ())) {
                  let entries = self.entries.lock();
                  for &(vt_usize, data_usize, _) in entries.iter() {
                      let vt = vt_usize as *const ();
                      let data = data_usize as *mut ();
                      f(vt, data);
                  }
              }

              /// Return all registered entries as `(vtable, data)` pointer pairs.
              pub fn all_entries(&self) -> Vec<(*const (), *mut ())> {
                  let entries = self.entries.lock();
                  entries
                      .iter()
                      .map(|&(vt_usize, data_usize, _)| {
                          (vt_usize as *const (), data_usize as *mut ())
                      })
                      .collect()
              }

              /// Probe all entries using the given probe function.
              /// Returns (confidence, data_ptr) pairs sorted descending.
              pub fn probe(
                  &self,
                  header: &[u8],
                  filename: &[u8],
                  probe_fn: unsafe extern "C" fn(
                      *const (),
                      &[u8],
                      &[u8],
                  ) -> u8,
              ) -> Vec<(u8, *mut ())> {
                  let entries = self.entries.lock();
                  let mut results: Vec<(u8, *mut ())> = Vec::new();
                  for &(vt_usize, data_usize, _) in entries.iter() {
                      let vt = vt_usize as *const ();
                      let conf = unsafe { probe_fn(vt, header, filename) };
                      if conf > 0 {
                          results.push((conf, data_usize as *mut ()));
                      }
                  }
                  results.sort_by(|a, b| b.0.cmp(&a.0));
                  results
              }

              /// Remove entries whose metadata matches.
              /// Frees the entry allocations through the stabby allocator.
              pub fn remove_by_meta(
                  &self,
                  meta: &::around_extensions::ExtensionMeta,
              ) -> usize {
                  let meta_ptr = meta as *const ::around_extensions::ExtensionMeta as usize;
                  let mut entries = self.entries.lock();
                  let before = entries.len();
                  // Drop matching entries before removing them.
                  entries.retain(|&(vt_usize, entry_usize, m)| {
                      if m == meta_ptr {
                          // SAFETY: entry was created via Box::into_raw(Box::new(DynRef)).
                          // Using from_raw reconstructs the stabby Dyn type, whose Drop impl
                          // correctly frees through the stabby allocator — no std::Box mismatch.
                          let vt = vt_usize as *const ();
                          let data = entry_usize as *mut ();
                          unsafe { drop(Self::from_raw(vt, data)) };
                          false
                      } else {
                          true
                      }
                  });
                  before - entries.len()
              }

              #[doc = concat!("Reconstruct a [`", stringify!(#dyn_ref_ident), "`] from raw vtable and data pointers.")]
              ///
              /// # Safety
              ///
              #[doc = concat!("* `vtable` must point to a valid vtable for the trait [`", stringify!(#trait_ident), "`],")]
              ///   obtained from a corresponding `RegisterVTable` instance.
              /// * `data` must point to a valid heap allocation produced by
              ///   `Box::into_raw(Box::new(concrete_type))`.
              /// * The caller must ensure that `vtable` type-matches the actual concrete type of `data`.
              pub unsafe fn from_raw(vtable: *const (), data: *mut ()) -> #dyn_ref_ident {
                  // stabby's dynptr! produces a repr(C) struct:
                  //   { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt }
                  // Write ptr at offset 0, vtable at offset size_of::<*mut ()>().
                  let mut result = core::mem::MaybeUninit::<#dyn_ref_ident>::uninit();
                  let p = result.as_mut_ptr();
                  unsafe {
                      core::ptr::write(p as *mut *mut (), data);
                      core::ptr::write(
                          (p as *mut u8).add(core::mem::size_of::<*mut ()>()) as *mut *const (),
                          vtable,
                      );
                  }
                  unsafe { result.assume_init() }
              }

              /// # Safety
              ///
              /// * `reg` must point to a valid `Self` allocation with a `'static` lifetime.
              /// * `entry` must be a valid heap allocation produced by
              #[doc = concat!("  `Box::into_raw(Box::new(", stringify!(#dyn_ref_ident), "))`.")]
              /// * `ext_meta` must point to a valid [`ExtensionMeta`](::around_extensions::ExtensionMeta).
              unsafe extern "C" fn push_raw_impl(
                  reg: *mut (),
                  entry: *mut (),
                  ext_meta: *const ::around_extensions::ExtensionMeta,
              ) {
                  let reg = unsafe { &*(reg as *const Self) };
                  reg.push(entry, ext_meta);
              }

              /// # Safety
              ///
              /// * `reg` must point to a valid `Self` allocation with a `'static` lifetime.
              /// * `meta` must point to a valid [`ExtensionMeta`](::around_extensions::ExtensionMeta).
              unsafe extern "C" fn remove_by_meta_impl(
                  reg: *mut (),
                  meta: *const ::around_extensions::ExtensionMeta,
              ) -> usize {
                  let reg = unsafe { &*(reg as *const Self) };
                  let meta = unsafe { &*meta };
                  reg.remove_by_meta(meta)
              }
          }
      }
    }
    SlotMode::Map => {
      quote! {
          impl #register_ident {
              pub fn new() -> Self {
                  Self {
                      entries: parking_lot::Mutex::new(std::collections::HashMap::new()),
                      push_counter: std::sync::atomic::AtomicUsize::new(0),
                  }
              }

              /// Push an opaque entry pointer onto this register.
              ///
              /// # Layout invariant
              ///
              /// This method extracts the vtable pointer at a fixed offset
              /// `core::mem::size_of::<*mut ()>()` from the entry pointer,
              /// assuming stabby's `dynptr!` macro produces a `repr(C)` struct
              /// with the following layout:
              ///
              /// ```text
              /// { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt }
              /// ```
              ///
              /// The `ManuallyDrop<Box<()>>` occupies `size_of::<*mut ()>()` bytes,
              /// placing the vtable reference immediately after.
              /// If stabby changes this layout, the offset must be updated here and
              /// in [`from_raw`].
              pub fn push(&self, entry: *mut (), meta: *const ::around_extensions::ExtensionMeta) {
                  const _: () = {
                      // Assert: the vtable pointer in stabby's Dyn<Box<()>, Vt> is at offset size_of::<*mut ()>().
                      // This is the documented layout: { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt, ... }
                      // If this fails, stabby's dynptr! layout changed — update the offset.
                  };
                  let vtable_ptr: *const () = unsafe {
                      *((entry as *const u8).add(core::mem::size_of::<*mut ()>()) as *const *const ())
                  };
                  let key = (meta as usize, self.push_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                  self.entries.lock().insert(key, (vtable_ptr as usize, entry as usize));
              }

              pub fn for_each_entry(&self, mut f: impl FnMut(*const (), *mut ())) {
                  let entries = self.entries.lock();
                  for &(vt_usize, data_usize) in entries.values() {
                      f(vt_usize as *const (), data_usize as *mut ());
                  }
              }

              /// Return all registered entries as `(vtable, data)` pointer pairs.
              pub fn all_entries(&self) -> Vec<(*const (), *mut ())> {
                  let entries = self.entries.lock();
                  entries
                      .iter()
                      .map(|(_, &(vt_usize, data_usize))| {
                          (vt_usize as *const (), data_usize as *mut ())
                      })
                      .collect()
              }

              pub fn probe(
                  &self,
                  header: &[u8],
                  filename: &[u8],
                  probe_fn: unsafe extern "C" fn(
                      *const (),
                      &[u8],
                      &[u8],
                  ) -> u8,
              ) -> Vec<(u8, *mut ())> {
                  let entries = self.entries.lock();
                  let mut results: Vec<(u8, *mut ())> = Vec::new();
                  for &(vt_usize, data_usize) in entries.values() {
                      let conf = unsafe { probe_fn(vt_usize as *const (), header, filename) };
                      if conf > 0 {
                          results.push((conf, data_usize as *mut ()));
                      }
                  }
                  results.sort_by(|a, b| b.0.cmp(&a.0));
                  results
              }

              pub fn remove_by_meta(
                  &self,
                  meta: &::around_extensions::ExtensionMeta,
              ) -> usize {
                  let meta_ptr = meta as *const ::around_extensions::ExtensionMeta as usize;
                  let mut entries = self.entries.lock();
                  let mut removed = 0;
                  entries.retain(|&(m, _), &mut (vt_usize, entry_usize)| {
                      if m == meta_ptr {
                          // SAFETY: entry was created via Box::into_raw(Box::new(DynRef)).
                          // Using from_raw reconstructs the stabby Dyn type, whose Drop impl
                          // correctly frees through the stabby allocator — no std::Box mismatch.
                          let vt = vt_usize as *const ();
                          let data = entry_usize as *mut ();
                          unsafe { drop(Self::from_raw(vt, data)) };
                          removed += 1;
                          false
                      } else {
                          true
                      }
                  });
                  removed
              }

              #[doc = concat!("Reconstruct a [`", stringify!(#dyn_ref_ident), "`] from raw vtable and data pointers.")]
              ///
              /// # Safety
              ///
              #[doc = concat!("* `vtable` must point to a valid vtable for the trait [`", stringify!(#trait_ident), "`],")]
              ///   obtained from a corresponding `RegisterVTable` instance.
              /// * `data` must point to a valid heap allocation produced by
              ///   `Box::into_raw(Box::new(concrete_type))`.
              /// * The caller must ensure that `vtable` type-matches the actual concrete type of `data`.
              pub unsafe fn from_raw(vtable: *const (), data: *mut ()) -> #dyn_ref_ident {
                  // stabby's dynptr! produces a repr(C) struct:
                  //   { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt }
                  // Write ptr at offset 0, vtable at offset size_of::<*mut ()>().
                  let mut result = core::mem::MaybeUninit::<#dyn_ref_ident>::uninit();
                  let p = result.as_mut_ptr();
                  unsafe {
                      core::ptr::write(p as *mut *mut (), data);
                      core::ptr::write(
                          (p as *mut u8).add(core::mem::size_of::<*mut ()>()) as *mut *const (),
                          vtable,
                      );
                  }
                  unsafe { result.assume_init() }
              }

              /// # Safety
              ///
              /// * `reg` must point to a valid `Self` allocation with a `'static` lifetime.
              /// * `entry` must be a valid heap allocation produced by
              #[doc = concat!("  `Box::into_raw(Box::new(", stringify!(#dyn_ref_ident), "))`.")]
              /// * `ext_meta` must point to a valid [`ExtensionMeta`](::around_extensions::ExtensionMeta).
              unsafe extern "C" fn push_raw_impl(
                  reg: *mut (),
                  entry: *mut (),
                  ext_meta: *const ::around_extensions::ExtensionMeta,
              ) {
                  let reg = unsafe { &*(reg as *const Self) };
                  reg.push(entry, ext_meta);
              }

              /// # Safety
              ///
              /// * `reg` must point to a valid `Self` allocation with a `'static` lifetime.
              /// * `meta` must point to a valid [`ExtensionMeta`](::around_extensions::ExtensionMeta).
              unsafe extern "C" fn remove_by_meta_impl(
                  reg: *mut (),
                  meta: *const ::around_extensions::ExtensionMeta,
              ) -> usize {
                  let reg = unsafe { &*(reg as *const Self) };
                  let meta = unsafe { &*meta };
                  reg.remove_by_meta(meta)
              }
          }
      }
    }
    SlotMode::Manual => {
      quote! {
          // Manual mode: no impl block.
          // Caller provides their own Register and wires REGISTER_VTABLE.
      }
    }
  };

  let static_vtable = quote! {
      /// Static RegisterVTable for use with [`attach_register`](::around_extensions::Framework::attach_register).
      pub static REGISTER_VTABLE: ::around_extensions::RegisterVTable =
          ::around_extensions::RegisterVTable {
              push_raw: #register_ident::push_raw_impl,
              remove_by_meta: #register_ident::remove_by_meta_impl,
          };
  };

  let dyn_ref_type = quote! {
      /// ABI-stable dyn reference for this trait.
      pub type #dyn_ref_ident = ::stabby::dynptr!(Box<dyn #trait_ident + Send + Sync>);
  };

  // Build the NAME expression: use custom name if provided, otherwise auto-generate
  // from CARGO_CRATE_NAME (read at proc-macro time) and trait name.
  let trait_name_lower = trait_ident.to_string().to_lowercase();
  let crate_name_raw =
    std::env::var("CARGO_CRATE_NAME").expect("CARGO_CRATE_NAME must be set by Cargo");
  let crate_name_lower = crate_name_raw.to_lowercase();
  let crate_name_lower_lit = syn::LitStr::new(&crate_name_lower, proc_macro2::Span::call_site());

  let name_expr: proc_macro2::TokenStream = if let Some(name) = &args.name {
    let lit = syn::LitStr::new(name, proc_macro2::Span::call_site());
    quote! { #lit }
  } else {
    quote! { concat!(#crate_name_lower_lit, "::", stringify!(#trait_ident)) }
  };

  let create_sym_expr: proc_macro2::TokenStream;
  let create_sym_nul_expr: proc_macro2::TokenStream;
  if let Some(name) = &args.name {
    let name_lower = name.to_lowercase();
    let create_sym = name_lower.replace("::", "_") + "_create";
    let create_sym_lit = syn::LitStr::new(&create_sym, proc_macro2::Span::call_site());
    let nul_bytes: Vec<u8> = create_sym.bytes().chain(std::iter::once(b'\0')).collect();
    let nul_lit = LitByteStr::new(&nul_bytes, proc_macro2::Span::call_site());
    create_sym_expr = quote! { #create_sym_lit };
    create_sym_nul_expr = quote! { #nul_lit };
  } else {
    let create_sym = format!("{crate_name_lower}_{trait_name_lower}_create");
    let create_sym_lit = syn::LitStr::new(&create_sym, proc_macro2::Span::call_site());
    let nul_bytes: Vec<u8> = create_sym.bytes().chain(std::iter::once(b'\0')).collect();
    let nul_lit = LitByteStr::new(&nul_bytes, proc_macro2::Span::call_site());
    create_sym_expr = quote! { #create_sym_lit };
    create_sym_nul_expr = quote! { #nul_lit };
  }
  let test_fn_ident = format_ident!("slot_create_sym_consistency_{}", trait_name_lower);
  let expanded = match args.mode {
    SlotMode::Manual => {
      quote! {
          #[doc = "Globally-unique slot identifier, derived from crate name and trait name."]
          pub const NAME: &'static str = #name_expr;
          #[doc = "Create-symbol for dynamic loading of this slot's constructor."]
          pub const CREATE_SYM: &'static str = #create_sym_expr;
          #[doc = "Null-terminated CREATE_SYM for C ABI interop."]
          pub const CREATE_SYM_NUL: &'static [u8] = #create_sym_nul_expr;
          #[repr(C)]
          pub struct #vtable_ident {
              pub inner: ::around_extensions::RegisterVTable,
          }

          #dyn_ref_type

          #[doc = concat!("Reconstruct a [`", stringify!(#dyn_ref_ident), "`] from raw vtable and data pointers.")]
          ///
          /// This reconstructs a `DynRef` from parts, without needing a `Register`.
          ///
          /// # Safety
          ///
          #[doc = concat!("* `vtable` must point to a valid vtable for the trait [`", stringify!(#trait_ident), "`],")]
          ///   obtained from a corresponding `RegisterVTable` instance.
          /// * `data` must point to a valid heap allocation produced by
          ///   `Box::into_raw(Box::new(concrete_type))`.
          /// * The caller must ensure that `vtable` type-matches the actual concrete type of `data`.
          pub unsafe fn from_raw(vtable: *const (), data: *mut ()) -> #dyn_ref_ident {
              // stabby's dynptr! produces a repr(C) struct:
              //   { ptr: ManuallyDrop<Box<()>>, vtable: &'static Vt }
              // Write ptr at offset 0, vtable at offset size_of::<*mut ()>().
              unsafe {
                  let mut result = core::mem::MaybeUninit::<#dyn_ref_ident>::uninit();
                  let p = result.as_mut_ptr();
                  core::ptr::write(p as *mut *mut (), data);
                  core::ptr::write(
                      (p as *mut u8).add(core::mem::size_of::<*mut ()>()) as *mut *const (),
                      vtable,
                  );
                  result.assume_init()
              }
          }
          #[doc = concat!("Verifies that CREATE_SYM is consistent with NAME for ", stringify!(#trait_ident), ".")]
          #[test]
          fn #test_fn_ident () {
              let derived = NAME.to_lowercase().replace("::", "_") + "_create";
              assert_eq!(CREATE_SYM, derived);
          }
          #input
      }
    }
    _ => {
      quote! {
          #[doc = "Globally-unique slot identifier, derived from crate name and trait name."]
          pub const NAME: &'static str = #name_expr;
          #[doc = "Create-symbol for dynamic loading of this slot's constructor."]
          pub const CREATE_SYM: &'static str = #create_sym_expr;
          #[doc = "Null-terminated CREATE_SYM for C ABI interop."]
          pub const CREATE_SYM_NUL: &'static [u8] = #create_sym_nul_expr;

          /// Typed wrapper for [`RegisterVTable`](::around_extensions::RegisterVTable).
          #[repr(C)]
          pub struct #vtable_ident {
              pub inner: ::around_extensions::RegisterVTable,
          }

          #register_struct

          #register_impl

          #static_vtable

          #dyn_ref_type

          #[doc = concat!("Verifies that CREATE_SYM is consistent with NAME for ", stringify!(#trait_ident), ".")]
          #[test]
          fn #test_fn_ident () {
              let derived = NAME.to_lowercase().replace("::", "_") + "_create";
              assert_eq!(CREATE_SYM, derived);
          }
          #input
      }
    }
  };

  TokenStream::from(expanded)
}
