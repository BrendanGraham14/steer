use std::path::{Path, PathBuf};

fn path_to_str(path: &Path) -> Result<&str, std::io::Error> {
    path.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("path is not valid UTF-8: {}", path.display()),
        )
    })
}

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

    let proto_dir_str = path_to_str(&proto_dir)?;
    let agent_proto_str = path_to_str(&agent_proto)?;
    let remote_workspace_proto_str = path_to_str(&remote_workspace_proto)?;
    let common_proto_str = path_to_str(&common_proto)?;

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc.clone());
    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .type_attribute(".", "#[allow(clippy::large_enum_variant)]")
        .compile_protos_with_config(config, &[common_proto_str], &[proto_dir_str])?;

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc.clone());
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .type_attribute(".", "#[allow(clippy::large_enum_variant)]")
        .compile_protos_with_config(
            config,
            &[agent_proto_str, remote_workspace_proto_str],
            &[proto_dir_str],
        )?;

    Ok(())
}
