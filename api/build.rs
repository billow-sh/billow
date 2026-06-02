fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=echo.proto");

    tonic_prost_build::compile_protos("echo.proto")?;

    Ok(())
}
