fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let proto_path = format!("{}/../../proto/keel.proto", manifest_dir);
    let proto_include = format!("{}/../../proto", manifest_dir);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/pb")
        .compile(&[&proto_path], &[&proto_include])?;

    Ok(())
}
