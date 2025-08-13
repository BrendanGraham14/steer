use steer_core::api::create_provider;
use steer_core::auth::ProviderRegistry;
use steer_core::auth::storage::Credential;
use steer_core::config::provider::{self, ApiFormat, AuthScheme, ProviderConfig, ProviderId};
use steer_core::config::toml_types::{Catalog, ProviderData};
use url::Url;

#[tokio::test]
async fn test_custom_provider_with_openai_format() {
    // Start a mock HTTP server
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    // Spawn mock server
    tokio::spawn(async move {
        let _guard = listener; // Keep listener alive
        // In a real test, we'd implement a proper mock server here
        // For now, we just keep the port bound
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    });

    // Create a custom provider config with OpenAI API format
    let provider_config = ProviderConfig {
        id: ProviderId("my-llm".to_string()),
        name: "My Custom LLM".to_string(),
        api_format: ApiFormat::OpenaiResponses,
        auth_schemes: vec![AuthScheme::ApiKey],
        base_url: Some(Url::parse(&base_url).unwrap()),
    };

    // Create credential
    let credential = Credential::ApiKey {
        value: "test-api-key".to_string(),
    };

    // Create provider using the factory
    let provider = create_provider(&provider_config, &credential).unwrap();

    // The provider should use the OpenAI client
    assert_eq!(provider.name(), "openai");

    // Test that the client would make requests to our custom URL
    // In a real test, we'd make an actual request and verify it hits our mock server
}

#[test]
fn test_provider_registry_custom_providers() {
    // Create a temporary directory for config
    let temp_dir = tempfile::tempdir().unwrap();

    // Write a custom catalog.toml with only providers
    let catalog = Catalog {
        providers: vec![
            ProviderData {
                id: "custom-openai".into(),
                name: "My OpenAI Compatible API".into(),
                api_format: ApiFormat::OpenaiResponses,
                auth_schemes: vec![AuthScheme::ApiKey],
                base_url: Some("https://my-api.example.com".into()),
            },
            ProviderData {
                id: "local-llm".into(),
                name: "Local LLM Server".into(),
                api_format: ApiFormat::OpenaiChat,
                auth_schemes: vec![AuthScheme::ApiKey],
                base_url: Some("http://localhost:8080".into()),
            },
        ],
        models: vec![],
    };

    let catalog_path = temp_dir.path().join("catalog.toml");
    std::fs::write(&catalog_path, toml::to_string(&catalog).unwrap()).unwrap();

    // Load registry with custom catalog
    let registry = ProviderRegistry::load(&[catalog_path.to_string_lossy().to_string()]).unwrap();

    // Check built-in providers still exist
    assert!(registry.get(&provider::anthropic()).is_some());
    assert!(registry.get(&provider::openai()).is_some());

    // Check custom providers were loaded
    let custom_openai = registry
        .get(&ProviderId("custom-openai".to_string()))
        .unwrap();
    assert_eq!(custom_openai.name, "My OpenAI Compatible API");
    assert_eq!(custom_openai.api_format, ApiFormat::OpenaiResponses);
    assert_eq!(
        custom_openai.base_url.as_ref().unwrap().as_str(),
        "https://my-api.example.com/"
    );

    let local_llm = registry.get(&ProviderId("local-llm".to_string())).unwrap();
    assert_eq!(local_llm.name, "Local LLM Server");
    assert_eq!(local_llm.api_format, ApiFormat::OpenaiChat);
    assert_eq!(
        local_llm.base_url.as_ref().unwrap().as_str(),
        "http://localhost:8080/"
    );
}

#[test]
fn test_xai_client_with_custom_base_url() {
    use steer_core::api::xai::XAIClient;

    let api_key = "test-key".to_string();
    let custom_url = "https://custom-xai-api.example.com".to_string();

    // Test with custom base URL
    let _client = XAIClient::with_base_url(api_key.clone(), Some(custom_url.clone()));
    // The client should be created successfully
    // In a real test, we'd verify it uses the custom URL for requests

    // Test without custom base URL (uses default)
    let _default_client = XAIClient::new(api_key);
    // Should use the default xAI API URL
}
