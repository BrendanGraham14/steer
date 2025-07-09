use crate::api::{grok::GrokClient, Model, Provider};
use conductor_tools::ToolSchema;
use tokio_util::sync::CancellationToken;

#[test]
fn test_grok_client_creation() {
    let api_key = "test_key".to_string();
    let client = GrokClient::new(api_key);
    assert_eq!(client.name(), "grok");
}

#[tokio::test]
async fn test_grok_client_provider_trait() {
    let api_key = "test_key".to_string();
    let client = GrokClient::new(api_key);
    
    // Test that the client implements Provider trait
    let _name: &str = client.name();
    assert_eq!(_name, "grok");
    
    // Test that we can call complete (will fail without valid API key, but tests compilation)
    let messages = vec![];
    let result = client.complete(
        Model::Grok3_20250910,
        messages,
        None,
        None::<Vec<ToolSchema>>,
        CancellationToken::new(),
    ).await;
    
    // We expect this to fail due to invalid API key
    assert!(result.is_err());
}