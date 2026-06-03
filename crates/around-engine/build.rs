fn main() -> std::io::Result<()> {
  #[cfg(feature = "protobuf-ipc")]
  {
    let mut config = prost_build::Config::new();
    config.out_dir(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()));
    // Generated types are not constructed until ProtoCodec is wired up.
    config.type_attribute(".", "#[allow(dead_code)]");
    config.compile_protos(&["proto/ipc.proto"], &["proto/"])?;
  }
  Ok(())
}
