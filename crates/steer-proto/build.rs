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

    let agent_proto = proto_dir.join("steer/agent/v1/agent.proto");
    let remote_workspace_proto = proto_dir.join("steer/remote_workspace/v1/remote_workspace.proto");
    let common_proto = proto_dir.join("steer/common/v1/common.proto");

    // Compile common proto first since others depend on it
    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .type_attribute(".", "#[allow(clippy::large_enum_variant)]")
        .compile_protos(
            &[common_proto.to_str().unwrap()],
            &[proto_dir.to_str().unwrap()],
        )?;

    // Compile agent and remote_workspace protos
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .type_attribute(".", "#[allow(clippy::large_enum_variant)]")
        .compile_protos(
            &[
                agent_proto.to_str().unwrap(),
                remote_workspace_proto.to_str().unwrap(),
            ],
            &[proto_dir.to_str().unwrap()],
        )?;
    Ok(())
}
