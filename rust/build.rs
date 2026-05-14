fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_file = "../proto/ledger.proto";
    let proto_dir = "../proto";

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&[proto_file], &[proto_dir])?;

    println!("cargo:rerun-if-changed={proto_file}");
    Ok(())
}
