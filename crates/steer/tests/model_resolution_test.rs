use std::fs;

use steer_core::catalog::CatalogConfig;
use steer_core::config::model::ModelId;
use steer_core::config::provider::ProviderId;
use steer_grpc::local_server::{LocalGrpcSetup, setup_local_grpc_with_catalog};
use steer_grpc::AgentClient;
use tempfile::TempDir;

async fn setup_client(catalog_paths: Vec<String>) -> (AgentClient, LocalGrpcSetup) {
    let setup = setup_local_grpc_with_catalog(
        steer_core::config::model::builtin::default_model(),
        None,
        CatalogConfig::with_catalogs(catalog_paths),
        None,
    )
    .await
    .expect("local grpc setup");
    let client = AgentClient::from_channel(setup.channel.clone())
        .await
        .expect("grpc client");
    (client, setup)
}

async fn shutdown(setup: LocalGrpcSetup) {
    setup.server_handle.abort();
    setup.runtime_service.shutdown().await;
}

async fn resolve_with_fallback(
    client: &AgentClient,
    preferred: Option<&str>,
) -> steer_core::config::model::ModelId {
    let server_default = client
        .get_default_model()
        .await
        .expect("server default model");

    if let Some(input) = preferred {
        match client.resolve_model(input).await {
            Ok(model_id) => model_id,
            Err(_) => server_default,
        }
    } else {
        server_default
    }
}

fn write_catalog(contents: &str) -> (TempDir, String) {
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("catalog.toml");
    fs::write(&path, contents).expect("write catalog");
    (tmp, path.to_string_lossy().to_string())
}

#[tokio::test]
async fn get_default_model_uses_builtin_when_recommended() {
    let (client, setup) = setup_client(Vec::new()).await;

    let default_model = client.get_default_model().await.expect("default model");
    let builtin_default = steer_core::config::model::builtin::default_model();
    assert_eq!(default_model, builtin_default);

    shutdown(setup).await;
}

#[tokio::test]
async fn get_default_model_prefers_recommended_override() {
    let catalog = r#"
[[providers]]
id = "aaa"
name = "AAA"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "openai"
id = "gpt-5.2-codex"
recommended = false

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
"#;
    let (_tmp, path) = write_catalog(catalog);
    let (client, setup) = setup_client(vec![path]).await;

    let default_model = client.get_default_model().await.expect("default model");
    assert_eq!(
        default_model,
        ModelId::new(ProviderId::from("aaa"), "aaa-model")
    );

    shutdown(setup).await;
}

#[tokio::test]
async fn preferred_model_wins_over_default() {
    let catalog = r#"
[[providers]]
id = "aaa"
name = "AAA"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "openai"
id = "gpt-5.2-codex"
recommended = false

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
"#;
    let (_tmp, path) = write_catalog(catalog);
    let (client, setup) = setup_client(vec![path]).await;

    let resolved = resolve_with_fallback(&client, Some("codex")).await;
    assert_eq!(
        resolved,
        ModelId::new(ProviderId::from("openai"), "gpt-5.2-codex")
    );

    shutdown(setup).await;
}

#[tokio::test]
async fn invalid_preference_falls_back_to_server_default() {
    let catalog = r#"
[[providers]]
id = "aaa"
name = "AAA"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "openai"
id = "gpt-5.2-codex"
recommended = false

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
"#;
    let (_tmp, path) = write_catalog(catalog);
    let (client, setup) = setup_client(vec![path]).await;

    let resolved = resolve_with_fallback(&client, Some("not-a-model")).await;
    assert_eq!(
        resolved,
        ModelId::new(ProviderId::from("aaa"), "aaa-model")
    );

    shutdown(setup).await;
}
