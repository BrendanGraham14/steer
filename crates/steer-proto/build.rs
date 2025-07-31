use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use vendored protoc to avoid requiring users to install it
    let protoc = protoc_bin_vendored::protoc_bin_path()?;

    // Get the manifest directory (where Cargo.toml is located)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let manifest_path = PathBuf::from(manifest_dir);

    // Use the proto directory within the crate
    let proto_dir = manifest_path.join("proto");

    println!("cargo:rerun-if-changed={}", proto_dir.display());

    let agent_proto = proto_dir.join("steer/agent/v1/agent.proto");
    let remote_workspace_proto = proto_dir.join("steer/remote_workspace/v1/remote_workspace.proto");
    let common_proto = proto_dir.join("steer/common/v1/common.proto");

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc.clone());
    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .type_attribute(".", "#[allow(clippy::large_enum_variant)]")
        .compile_protos_with_config(
            config,
            &[common_proto.to_str().unwrap()],
            &[proto_dir.to_str().unwrap()],
        )?;

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc.clone());
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .type_attribute(".", "#[allow(clippy::large_enum_variant)]")
        .compile_protos_with_config(
            config,
            &[
                agent_proto.to_str().unwrap(),
                remote_workspace_proto.to_str().unwrap(),
            ],
            &[proto_dir.to_str().unwrap()],
        )?;

    Ok(())
}
