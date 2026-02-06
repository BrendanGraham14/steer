use std::fs;

use steer_core::catalog::CatalogConfig;
use steer_core::config::model::ModelId;
use steer_core::config::provider::ProviderId;
use steer_grpc::AgentClient;
use steer_grpc::local_server::{LocalGrpcSetup, setup_local_grpc_with_catalog};
use tempfile::TempDir;
use thiserror::Error;

type TestResult<T> = Result<T, TestError>;

#[derive(Debug, Error)]
enum TestError {
    #[error(transparent)]
    Grpc(#[from] steer_grpc::GrpcError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

async fn setup_client(catalog_paths: Vec<String>) -> TestResult<(AgentClient, LocalGrpcSetup)> {
    let setup = setup_local_grpc_with_catalog(
        steer_core::config::model::builtin::default_model(),
        None,
        CatalogConfig::with_catalogs(catalog_paths),
        None,
    )
    .await?;
    let client = AgentClient::from_channel(setup.channel.clone()).await?;
    Ok((client, setup))
}

async fn shutdown(setup: LocalGrpcSetup) {
    setup.server_handle.abort();
    setup.runtime_service.shutdown().await;
}

async fn resolve_with_fallback(
    client: &AgentClient,
    preferred: Option<&str>,
) -> TestResult<steer_core::config::model::ModelId> {
    let server_default = client.get_default_model().await?;

    let resolved = if let Some(input) = preferred {
        match client.resolve_model(input).await {
            Ok(model_id) => model_id,
            Err(_) => server_default,
        }
    } else {
        server_default
    };

    Ok(resolved)
}

fn write_catalog(contents: &str) -> TestResult<(TempDir, String)> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("catalog.toml");
    fs::write(&path, contents)?;
    Ok((tmp, path.to_string_lossy().to_string()))
}

#[tokio::test]
async fn get_default_model_uses_builtin_when_recommended() {
    let (client, setup) = setup_client(Vec::new()).await.expect("setup client");

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
id = "gpt-5.3-codex"
recommended = false

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
"#;
    let (_tmp, path) = write_catalog(catalog).expect("write catalog");
    let (client, setup) = setup_client(vec![path]).await.expect("setup client");

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
id = "gpt-5.3-codex"
recommended = false

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
"#;
    let (_tmp, path) = write_catalog(catalog).expect("write catalog");
    let (client, setup) = setup_client(vec![path]).await.expect("setup client");

    let resolved = resolve_with_fallback(&client, Some("codex"))
        .await
        .expect("resolve model");
    assert_eq!(
        resolved,
        ModelId::new(ProviderId::from("openai"), "gpt-5.3-codex")
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
id = "gpt-5.3-codex"
recommended = false

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
"#;
    let (_tmp, path) = write_catalog(catalog).expect("write catalog");
    let (client, setup) = setup_client(vec![path]).await.expect("setup client");

    let resolved = resolve_with_fallback(&client, Some("not-a-model"))
        .await
        .expect("resolve model");
    assert_eq!(resolved, ModelId::new(ProviderId::from("aaa"), "aaa-model"));

    shutdown(setup).await;
}
