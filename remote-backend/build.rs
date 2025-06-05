fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false) // Agent only needs server
        .compile_protos(&["../proto/remote_backend.proto"], &["../proto"])?;
    Ok(())
}
