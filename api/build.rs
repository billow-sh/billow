fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=api.proto");

    tonic_prost_build::compile_protos("api.proto")?;

    Ok(())
}
