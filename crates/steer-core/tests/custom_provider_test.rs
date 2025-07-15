use steer_core::api::create_provider;
use steer_core::auth::ProviderRegistry;
use steer_core::auth::storage::Credential;
use steer_core::config::provider::{ApiFormat, AuthScheme, Provider as ProviderId, ProviderConfig};
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
        id: ProviderId::Custom("my-llm".to_string()),
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

    // Write a custom providers.toml
    let providers_toml = r#"
[[providers]]
id = { custom = "custom-openai" }
name = "My OpenAI Compatible API"
api_format = "openai-responses"
auth_schemes = ["api_key"]
base_url = "https://my-api.example.com"

[[providers]]
id = { custom = "local-llm" }
name = "Local LLM Server"  
api_format = "openai-chat"
auth_schemes = ["api_key"]
base_url = "http://localhost:8080"
"#;

    let steer_dir = temp_dir.path().join("conductor");
    std::fs::create_dir_all(&steer_dir).unwrap();
    std::fs::write(steer_dir.join("providers.toml"), providers_toml).unwrap();

    // Load registry with custom config
    let registry = ProviderRegistry::load_with_config_dir(Some(temp_dir.path())).unwrap();

    // Check built-in providers still exist
    assert!(registry.get(&ProviderId::Anthropic).is_some());
    assert!(registry.get(&ProviderId::Openai).is_some());

    // Check custom providers were loaded
    let custom_openai = registry
        .get(&ProviderId::Custom("custom-openai".to_string()))
        .unwrap();
    assert_eq!(custom_openai.name, "My OpenAI Compatible API");
    assert_eq!(custom_openai.api_format, ApiFormat::OpenaiResponses);
    assert_eq!(
        custom_openai.base_url.as_ref().unwrap().as_str(),
        "https://my-api.example.com/"
    );

    let local_llm = registry
        .get(&ProviderId::Custom("local-llm".to_string()))
        .unwrap();

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
