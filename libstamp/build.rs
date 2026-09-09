//! Build script for `libstamp`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()
        .map_err(|e| format!("Failed to find protoc bin: {e}"))?;
    unsafe {
        std::env::set_var("PROTOC", protoc_path);
    }
    tonic_build::compile_protos("proto/packer.proto")?;
    Ok(())
}
