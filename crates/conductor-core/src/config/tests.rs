#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::test_utils::InMemoryAuthStorage;

    #[tokio::test]
    async fn test_auth_changes_immediately_reflected() {
        // Create a provider with in-memory storage
        let storage = Arc::new(InMemoryAuthStorage::new());
        let provider = LlmConfigProvider::new(storage.clone());

        // Initially no auth
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(auth.is_none());

        // Add API key
        storage
            .set_credential(
                "anthropic",
                crate::auth::Credential::ApiKey {
                    value: "test-key".to_string(),
                },
            )
            .await
            .unwrap();

        // Should immediately see the new key
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(matches!(auth, Some(ApiAuth::Key(key)) if key == "test-key"));

        // Add OAuth tokens
        storage
            .set_credential(
                "anthropic",
                crate::auth::Credential::OAuth2(crate::auth::storage::OAuth2Token {
                    access_token: "access".to_string(),
                    refresh_token: "refresh".to_string(),
                    expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
                }),
            )
            .await
            .unwrap();

        // Should immediately prefer OAuth over API key
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(matches!(auth, Some(ApiAuth::OAuth)));

        // Remove OAuth tokens
        storage
            .remove_credential("anthropic", crate::auth::CredentialType::OAuth2)
            .await
            .unwrap();

        // Should immediately fall back to API key
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(matches!(auth, Some(ApiAuth::Key(key)) if key == "test-key"));
    }

    #[tokio::test]
    async fn test_available_providers_updates_immediately() {
        let storage = Arc::new(InMemoryAuthStorage::new());
        let provider = LlmConfigProvider::new(storage.clone());

        // Initially no providers
        let providers = provider.available_providers().await.unwrap();
        assert!(providers.is_empty());

        // Add Anthropic API key
        storage
            .set_credential(
                "anthropic",
                crate::auth::Credential::ApiKey {
                    value: "test-key".to_string(),
                },
            )
            .await
            .unwrap();

        // Should immediately show Anthropic
        let providers = provider.available_providers().await.unwrap();
        assert_eq!(providers, vec![ProviderKind::Anthropic]);

        // Add OpenAI key
        storage
            .set_credential(
                "openai",
                crate::auth::Credential::ApiKey {
                    value: "openai-key".to_string(),
                },
            )
            .await
            .unwrap();

        // Should immediately show both
        let providers = provider.available_providers().await.unwrap();
        assert_eq!(
            providers,
            vec![ProviderKind::Anthropic, ProviderKind::OpenAI]
        );
    }
}
