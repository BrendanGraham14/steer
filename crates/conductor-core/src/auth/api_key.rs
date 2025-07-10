use crate::api::ProviderKind;
use crate::auth::{AuthError, AuthStorage, Credential, CredentialType, Result};
use crate::auth::{AuthMethod, AuthProgress, AuthenticationFlow};
use async_trait::async_trait;
use std::sync::Arc;

/// Generic API key authentication flow for providers that support API keys
pub struct ApiKeyAuthFlow {
    storage: Arc<dyn AuthStorage>,
    provider: ProviderKind,
}

impl ApiKeyAuthFlow {
    pub fn new(storage: Arc<dyn AuthStorage>, provider: ProviderKind) -> Self {
        Self { storage, provider }
    }

    /// Validate an API key format based on provider-specific rules
    fn validate_api_key(&self, api_key: &str) -> Result<()> {
        let trimmed = api_key.trim();

        if trimmed.is_empty() {
            return Err(AuthError::InvalidCredential(
                "API key cannot be empty".to_string(),
            ));
        }

        // Provider-specific validation
        match self.provider {
            ProviderKind::OpenAI => {
                if !trimmed.starts_with("sk-") || trimmed.len() < 20 {
                    return Err(AuthError::InvalidCredential(
                        "OpenAI API keys should start with 'sk-' and be at least 20 characters"
                            .to_string(),
                    ));
                }
            }
            ProviderKind::Anthropic => {
                if !trimmed.starts_with("sk-ant-") {
                    return Err(AuthError::InvalidCredential(
                        "Anthropic API keys should start with 'sk-ant-'".to_string(),
                    ));
                }
            }
            ProviderKind::Google => {
                // Google/Gemini keys are typically 39 characters
                if trimmed.len() < 30 {
                    return Err(AuthError::InvalidCredential(
                        "Google API key appears to be too short".to_string(),
                    ));
                }
            }
            ProviderKind::Grok => {
                // Grok doesn't have a specific format requirement yet
                if trimmed.len() < 10 {
                    return Err(AuthError::InvalidCredential(
                        "API key appears to be too short".to_string(),
                    ));
                }
            }
        }

        // Check for common mistakes
        if trimmed.contains(' ') && !trimmed.contains("Bearer") {
            return Err(AuthError::InvalidCredential(
                "API key should not contain spaces".to_string(),
            ));
        }

        Ok(())
    }
}

/// State for the API key authentication flow
#[derive(Debug, Clone)]
pub struct ApiKeyAuthState {
    pub awaiting_input: bool,
}

#[async_trait]
impl AuthenticationFlow for ApiKeyAuthFlow {
    type State = ApiKeyAuthState;

    fn available_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::ApiKey]
    }

    async fn start_auth(&self, method: AuthMethod) -> Result<Self::State> {
        match method {
            AuthMethod::ApiKey => Ok(ApiKeyAuthState {
                awaiting_input: true,
            }),
            _ => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: self.provider,
            }),
        }
    }

    async fn get_initial_progress(
        &self,
        _state: &Self::State,
        method: AuthMethod,
    ) -> Result<AuthProgress> {
        match method {
            AuthMethod::ApiKey => Ok(AuthProgress::NeedInput(format!(
                "Enter your {} API key",
                self.provider
            ))),
            _ => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: self.provider,
            }),
        }
    }

    async fn handle_input(&self, state: &mut Self::State, input: &str) -> Result<AuthProgress> {
        if !state.awaiting_input {
            return Err(AuthError::InvalidState(
                "Not expecting input at this stage".to_string(),
            ));
        }

        // Validate the API key
        self.validate_api_key(input)?;

        // Store the API key
        self.storage
            .set_credential(
                &self.provider.to_string(),
                Credential::ApiKey {
                    value: input.trim().to_string(),
                },
            )
            .await
            .map_err(|e| AuthError::Storage(format!("Failed to store API key: {e}")))?;

        state.awaiting_input = false;
        Ok(AuthProgress::Complete)
    }

    async fn is_authenticated(&self) -> Result<bool> {
        Ok(self
            .storage
            .get_credential(&self.provider.to_string(), CredentialType::ApiKey)
            .await?
            .is_some())
    }

    fn provider_name(&self) -> String {
        self.provider.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthStorage, Credential, CredentialType};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    /// Mock implementation of AuthStorage for testing
    struct MockAuthStorage {
        credentials: Arc<Mutex<HashMap<(String, CredentialType), Credential>>>,
    }

    impl MockAuthStorage {
        fn new() -> Self {
            Self {
                credentials: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl AuthStorage for MockAuthStorage {
        async fn get_credential(
            &self,
            provider: &str,
            credential_type: CredentialType,
        ) -> Result<Option<Credential>> {
            let creds = self.credentials.lock().await;
            Ok(creds.get(&(provider.to_string(), credential_type)).cloned())
        }

        async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
            let mut creds = self.credentials.lock().await;
            let cred_type = match &credential {
                Credential::ApiKey { .. } => CredentialType::ApiKey,
                Credential::AuthTokens { .. } => CredentialType::AuthTokens,
            };
            creds.insert((provider.to_string(), cred_type), credential);
            Ok(())
        }

        async fn remove_credential(
            &self,
            provider: &str,
            credential_type: CredentialType,
        ) -> Result<()> {
            let mut creds = self.credentials.lock().await;
            creds.remove(&(provider.to_string(), credential_type));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_api_key_flow() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = ApiKeyAuthFlow::new(storage.clone(), ProviderKind::Grok);

        // Test available methods
        let methods = auth_flow.available_methods();
        assert_eq!(methods.len(), 1);
        assert!(methods.contains(&AuthMethod::ApiKey));

        // Start API key flow
        let state = auth_flow.start_auth(AuthMethod::ApiKey).await.unwrap();
        assert!(state.awaiting_input);

        // Get initial progress
        let progress = auth_flow
            .get_initial_progress(&state, AuthMethod::ApiKey)
            .await
            .unwrap();
        match progress {
            AuthProgress::NeedInput(msg) => assert_eq!(msg, "Enter your grok API key"),
            _ => panic!("Expected NeedInput progress"),
        }

        // Handle API key input
        let mut state = state;
        let progress = auth_flow
            .handle_input(&mut state, "test-api-key-12345")
            .await
            .unwrap();
        assert!(matches!(progress, AuthProgress::Complete));
        assert!(!state.awaiting_input);

        // Verify API key was stored
        let cred = storage
            .get_credential(&ProviderKind::Grok.to_string(), CredentialType::ApiKey)
            .await
            .unwrap();
        assert!(cred.is_some());
        if let Some(Credential::ApiKey { value }) = cred {
            assert_eq!(value, "test-api-key-12345");
        } else {
            panic!("Expected API key credential");
        }

        // Verify authentication status
        assert!(auth_flow.is_authenticated().await.unwrap());
    }

    #[tokio::test]
    async fn test_empty_api_key() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = ApiKeyAuthFlow::new(storage, ProviderKind::Grok);

        let mut state = auth_flow.start_auth(AuthMethod::ApiKey).await.unwrap();

        // Test empty input
        let result = auth_flow.handle_input(&mut state, "").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthError::InvalidCredential(msg) => {
                assert_eq!(msg, "API key cannot be empty");
            }
            _ => panic!("Expected InvalidCredential error"),
        }
    }

    #[tokio::test]
    async fn test_invalid_method() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = ApiKeyAuthFlow::new(storage, ProviderKind::Grok);

        // Test with OAuth method
        let result = auth_flow.start_auth(AuthMethod::OAuth).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthError::UnsupportedMethod { method, provider } => {
                assert_eq!(method, "OAuth");
                assert_eq!(provider, ProviderKind::Grok);
            }
            _ => panic!("Expected UnsupportedMethod error"),
        }
    }

    #[tokio::test]
    async fn test_openai_key_validation() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = ApiKeyAuthFlow::new(storage, ProviderKind::OpenAI);

        let mut state = auth_flow.start_auth(AuthMethod::ApiKey).await.unwrap();

        // Test invalid OpenAI key format
        let result = auth_flow.handle_input(&mut state, "invalid-key").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthError::InvalidCredential(msg) => {
                assert!(msg.contains("OpenAI API keys should start with 'sk-'"));
            }
            _ => panic!("Expected InvalidCredential error"),
        }

        // Test valid OpenAI key format
        let result = auth_flow
            .handle_input(&mut state, "sk-1234567890abcdef1234567890")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_api_key_with_spaces() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = ApiKeyAuthFlow::new(storage, ProviderKind::Grok);

        let mut state = auth_flow.start_auth(AuthMethod::ApiKey).await.unwrap();

        // Test API key with spaces
        let result = auth_flow
            .handle_input(&mut state, "test key with spaces")
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthError::InvalidCredential(msg) => {
                assert_eq!(msg, "API key should not contain spaces");
            }
            _ => panic!("Expected InvalidCredential error"),
        }
    }
}
