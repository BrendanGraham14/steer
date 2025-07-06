use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get the manifest directory (where Cargo.toml is located)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let manifest_path = PathBuf::from(manifest_dir);

    // Navigate to the proto directory from the crate root
    let proto_dir = manifest_path
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .map(|p| p.join("proto"))
        .expect("Failed to find proto directory");

    println!("cargo:rerun-if-changed={}", proto_dir.display());

    let streaming_proto = proto_dir.join("streaming.proto");
    let remote_workspace_proto = proto_dir.join("remote_workspace.proto");

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                streaming_proto.to_str().unwrap(),
                remote_workspace_proto.to_str().unwrap(),
            ],
            &[proto_dir.to_str().unwrap()],
        )?;
    Ok(())
}
