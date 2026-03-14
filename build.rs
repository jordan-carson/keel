fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .out_dir("src/pb")
        .compile(&["proto/keel.proto"], &["proto"])?;

    // Also generate for client use
    tonic_build::configure()
        .build_client(true)
        .out_dir("src/pb")
        .compile(&["proto/keel.proto"], &["proto"])?;

    Ok(())
}
