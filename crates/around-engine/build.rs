// OUT_DIR is always set by cargo during builds; panicking is the correct behavior if missing.
#[expect(
  clippy::expect_used,
  reason = "OUT_DIR is always set by Cargo during compilation. If missing, the build environment is catastrophically broken and panicking is correct."
)]
fn main() -> std::io::Result<()> {
  #[cfg(feature = "protobuf-ipc")]
  {
    let mut config = prost_build::Config::new();
    config.out_dir(std::path::PathBuf::from(
      std::env::var("OUT_DIR").expect("OUT_DIR must be set by cargo"),
    ));
    // This #[allow(dead_code)] is applied to all auto-generated protobuf types
    // from ipc.proto. These types are not constructed until the protobuf IPC
    // codec is fully wired (ADR-0003). Once ProtoCodec is complete and all
    // generated types are used, this attribute can be removed.
    config.type_attribute(".", "#[expect(dead_code, reason = \"protobuf-generated types from prost-build; fields are read by generated serialize/deserialize code, not directly in hand-written Rust\")]");
    config.compile_protos(&["proto/ipc.proto"], &["proto/"])?;
  }
  Ok(())
}
