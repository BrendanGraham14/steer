pub mod claude;
pub mod error;
pub mod factory;
pub mod gemini;
pub mod openai;
pub mod provider;
pub mod sse;
pub mod util;
pub mod xai;

use crate::auth::ProviderRegistry;
use crate::auth::storage::{Credential, CredentialType};
use crate::config::model::ModelId;
use crate::config::provider::ProviderId;
use crate::config::{ApiAuth, LlmConfigProvider};
use crate::error::Result;
use crate::model_registry::ModelRegistry;
pub use error::{ApiError, StreamError};
pub use factory::{create_provider, create_provider_with_storage};
pub use provider::{CompletionResponse, CompletionStream, Provider, StreamChunk};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use steer_tools::ToolSchema;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::app::conversation::Message;

#[derive(Clone)]
pub struct Client {
    provider_map: Arc<RwLock<HashMap<ProviderId, Arc<dyn Provider>>>>,
    config_provider: LlmConfigProvider,
    provider_registry: Arc<ProviderRegistry>,
    model_registry: Arc<ModelRegistry>,
}

impl Client {
    /// Remove a cached provider so that future calls re-create it with fresh credentials.
    fn invalidate_provider(&self, provider_id: &ProviderId) {
        let mut map = self.provider_map.write().unwrap();
        map.remove(provider_id);
    }

    /// Determine if an API error should invalidate the cached provider (typically auth failures).
    fn should_invalidate_provider(error: &ApiError) -> bool {
        matches!(
            error,
            ApiError::AuthenticationFailed { .. } | ApiError::AuthError(_)
        ) || matches!(
            error,
            ApiError::ServerError { status_code, .. } if matches!(status_code, 401 | 403)
        )
    }

    /// Create a new Client with all dependencies injected.
    /// This is the preferred constructor to avoid internal registry loading.
    pub fn new_with_deps(
        config_provider: LlmConfigProvider,
        provider_registry: Arc<ProviderRegistry>,
        model_registry: Arc<ModelRegistry>,
    ) -> Self {
        Self {
            provider_map: Arc::new(RwLock::new(HashMap::new())),
            config_provider,
            provider_registry,
            model_registry,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn insert_test_provider(&self, provider_id: ProviderId, provider: Arc<dyn Provider>) {
        let mut map = self.provider_map.write().unwrap();
        map.insert(provider_id, provider);
    }

    async fn get_or_create_provider(&self, provider_id: ProviderId) -> Result<Arc<dyn Provider>> {
        // First check without holding the lock across await
        {
            let map = self.provider_map.read().unwrap();
            if let Some(provider) = map.get(&provider_id) {
                return Ok(provider.clone());
            }
        }

        // Get the provider config from registry
        let provider_config = self.provider_registry.get(&provider_id).ok_or_else(|| {
            crate::error::Error::Api(ApiError::Configuration(format!(
                "No provider configuration found for {provider_id:?}"
            )))
        })?;

        // Get credential for the provider
        let credential = match self
            .config_provider
            .get_auth_for_provider(&provider_id)
            .await?
        {
            Some(ApiAuth::OAuth) => {
                // Get OAuth credential from storage using the centralized storage_key()
                self.config_provider
                    .auth_storage()
                    .get_credential(&provider_id.storage_key(), CredentialType::OAuth2)
                    .await
                    .map_err(|e| {
                        crate::error::Error::Api(ApiError::Configuration(format!(
                            "Failed to get OAuth credential: {e}"
                        )))
                    })?
                    .ok_or_else(|| {
                        crate::error::Error::Api(ApiError::Configuration(
                            "OAuth credential not found in storage".to_string(),
                        ))
                    })?
            }
            Some(ApiAuth::Key(key)) => Credential::ApiKey { value: key },
            None => {
                return Err(crate::error::Error::Api(ApiError::Configuration(format!(
                    "No authentication configured for {provider_id:?}"
                ))));
            }
        };

        // Now acquire write lock and create provider
        let mut map = self.provider_map.write().unwrap();

        // Check again in case another thread added it
        if let Some(provider) = map.get(&provider_id) {
            return Ok(provider.clone());
        }

        // Create the provider using factory
        let provider_instance = if matches!(&credential, Credential::OAuth2(_)) {
            factory::create_provider_with_storage(
                provider_config,
                &credential,
                self.config_provider.auth_storage().clone(),
            )
            .map_err(crate::error::Error::Api)?
        } else {
            factory::create_provider(provider_config, &credential)
                .map_err(crate::error::Error::Api)?
        };

        map.insert(provider_id, provider_instance.clone());
        Ok(provider_instance)
    }

    /// Complete a prompt with a specific model ID and optional parameters
    pub async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<crate::config::model::ModelParameters>,
        token: CancellationToken,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        // Get provider from model ID
        let provider_id = model_id.0.clone();
        let provider = self
            .get_or_create_provider(provider_id.clone())
            .await
            .map_err(ApiError::from)?;

        if token.is_cancelled() {
            return Err(ApiError::Cancelled {
                provider: provider.name().to_string(),
            });
        }

        // Get model config and merge parameters
        let model_config = self.model_registry.get(model_id);
        let effective_params = match (model_config, &call_options) {
            (Some(config), Some(opts)) => config.effective_parameters(Some(opts)),
            (Some(config), None) => config.effective_parameters(None),
            (None, Some(opts)) => Some(*opts),
            (None, None) => None,
        };

        debug!(
            target: "api::complete",
            ?model_id,
            ?call_options,
            ?effective_params,
            "Final parameters for model"
        );

        let result = provider
            .complete(model_id, messages, system, tools, effective_params, token)
            .await;

        if let Err(ref err) = result {
            if Self::should_invalidate_provider(err) {
                self.invalidate_provider(&provider_id);
            }
        }

        result
    }

    pub async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<crate::config::model::ModelParameters>,
        token: CancellationToken,
    ) -> std::result::Result<CompletionStream, ApiError> {
        let provider_id = model_id.0.clone();
        let provider = self
            .get_or_create_provider(provider_id.clone())
            .await
            .map_err(ApiError::from)?;

        if token.is_cancelled() {
            return Err(ApiError::Cancelled {
                provider: provider.name().to_string(),
            });
        }

        let model_config = self.model_registry.get(model_id);
        let effective_params = match (model_config, &call_options) {
            (Some(config), Some(opts)) => config.effective_parameters(Some(opts)),
            (Some(config), None) => config.effective_parameters(None),
            (None, Some(opts)) => Some(*opts),
            (None, None) => None,
        };

        debug!(
            target: "api::stream_complete",
            ?model_id,
            ?call_options,
            ?effective_params,
            "Streaming with parameters"
        );

        let result = provider
            .stream_complete(model_id, messages, system, tools, effective_params, token)
            .await;

        if let Err(ref err) = result {
            if Self::should_invalidate_provider(err) {
                self.invalidate_provider(&provider_id);
            }
        }

        result
    }

    pub async fn complete_with_retry(
        &self,
        model_id: &ModelId,
        messages: &[Message],
        system_prompt: &Option<String>,
        tools: &Option<Vec<ToolSchema>>,
        token: CancellationToken,
        max_attempts: usize,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        let mut attempts = 0;

        // Prepare provider and parameters once
        let provider_id = model_id.0.clone();
        let provider = self
            .get_or_create_provider(provider_id.clone())
            .await
            .map_err(ApiError::from)?;

        let model_config = self.model_registry.get(model_id);
        debug!(
            target: "api::complete_with_retry",
            ?model_id,
            ?model_config,
            "Model config"
        );
        let effective_params = model_config.and_then(|cfg| cfg.effective_parameters(None));

        debug!(
            target: "api::complete_with_retry",
            ?model_id,
            ?effective_params,
            "system: {:?}",
            system_prompt
        );
        debug!(
            target: "api::complete_with_retry",
            ?model_id,
            "messages: {:?}",
            messages
        );

        loop {
            if token.is_cancelled() {
                return Err(ApiError::Cancelled {
                    provider: provider.name().to_string(),
                });
            }

            match provider
                .complete(
                    model_id,
                    messages.to_vec(),
                    system_prompt.clone(),
                    tools.clone(),
                    effective_params,
                    token.clone(),
                )
                .await
            {
                Ok(response) => {
                    return Ok(response);
                }
                Err(error) => {
                    attempts += 1;
                    warn!(
                        "API completion attempt {}/{} failed for model {:?}: {:?}",
                        attempts, max_attempts, model_id, error
                    );

                    if Self::should_invalidate_provider(&error) {
                        self.invalidate_provider(&provider_id);
                        return Err(error);
                    }

                    if attempts >= max_attempts {
                        return Err(error);
                    }

                    match error {
                        ApiError::RateLimited { provider, details } => {
                            let sleep_duration =
                                std::time::Duration::from_secs(1 << (attempts - 1));
                            warn!(
                                "Rate limited by API: {} {} (retrying in {} seconds)",
                                provider,
                                details,
                                sleep_duration.as_secs()
                            );
                            tokio::time::sleep(sleep_duration).await;
                        }
                        ApiError::NoChoices { provider } => {
                            warn!("No choices returned from API: {}", provider);
                        }
                        ApiError::ServerError {
                            provider,
                            status_code,
                            details,
                        } => {
                            warn!(
                                "Server error for API: {} {} {}",
                                provider, status_code, details
                            );
                        }
                        _ => {
                            // Not retryable
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider::ProviderId;
    use async_trait::async_trait;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Copy)]
    enum StubErrorKind {
        Auth,
        Server401,
    }

    #[derive(Clone)]
    struct StubProvider {
        error_kind: StubErrorKind,
    }

    impl StubProvider {
        fn new(error_kind: StubErrorKind) -> Self {
            Self { error_kind }
        }
    }

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<String>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> std::result::Result<CompletionResponse, ApiError> {
            let err = match self.error_kind {
                StubErrorKind::Auth => ApiError::AuthenticationFailed {
                    provider: "stub".to_string(),
                    details: "bad key".to_string(),
                },
                StubErrorKind::Server401 => ApiError::ServerError {
                    provider: "stub".to_string(),
                    status_code: 401,
                    details: "unauthorized".to_string(),
                },
            };
            Err(err)
        }
    }

    fn test_client() -> Client {
        let auth_storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        let config_provider = LlmConfigProvider::new(auth_storage);
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));

        Client::new_with_deps(config_provider, provider_registry, model_registry)
    }

    fn insert_stub_provider(client: &Client, provider_id: ProviderId, error: StubErrorKind) {
        client
            .provider_map
            .write()
            .unwrap()
            .insert(provider_id, Arc::new(StubProvider::new(error)));
    }

    #[tokio::test]
    async fn invalidates_cached_provider_on_auth_failure() {
        let client = test_client();
        let provider_id = ProviderId("stub-auth".to_string());
        let model_id = (provider_id.clone(), "stub-model".to_string());

        insert_stub_provider(&client, provider_id.clone(), StubErrorKind::Auth);

        let err = client
            .complete(
                &model_id,
                vec![],
                None,
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ApiError::AuthenticationFailed { .. }));
        assert!(
            !client
                .provider_map
                .read()
                .unwrap()
                .contains_key(&provider_id)
        );
    }

    #[tokio::test]
    async fn invalidates_cached_provider_on_unauthorized_status_code() {
        let client = test_client();
        let provider_id = ProviderId("stub-unauthorized".to_string());
        let model_id = (provider_id.clone(), "stub-model".to_string());

        insert_stub_provider(&client, provider_id.clone(), StubErrorKind::Server401);

        let err = client
            .complete(
                &model_id,
                vec![],
                None,
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ApiError::ServerError {
                status_code: 401,
                ..
            }
        ));
        assert!(
            !client
                .provider_map
                .read()
                .unwrap()
                .contains_key(&provider_id)
        );
    }
}
