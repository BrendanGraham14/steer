pub mod claude;
pub mod error;
pub mod factory;
pub mod gemini;
pub mod openai;
pub mod provider;
pub mod sse;
pub mod util;
pub mod xai;

use crate::auth::storage::Credential;
use crate::auth::{AuthSource, ProviderRegistry};
use crate::config::model::ModelId;
use crate::config::provider::ProviderId;
use crate::config::{LlmConfigProvider, ResolvedAuth};
use crate::error::Result;
use crate::model_registry::ModelRegistry;
pub use error::{ApiError, SseParseError, StreamError};
pub use factory::{create_provider, create_provider_with_directive};
pub use provider::{CompletionResponse, CompletionStream, Provider, StreamChunk};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use steer_tools::ToolSchema;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::app::SystemContext;
use crate::app::conversation::Message;

#[derive(Clone)]
pub struct Client {
    provider_map: Arc<RwLock<HashMap<ProviderId, ProviderEntry>>>,
    config_provider: LlmConfigProvider,
    provider_registry: Arc<ProviderRegistry>,
    model_registry: Arc<ModelRegistry>,
}

#[derive(Clone)]
struct ProviderEntry {
    provider: Arc<dyn Provider>,
    auth_source: AuthSource,
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
        map.insert(
            provider_id,
            ProviderEntry {
                provider,
                auth_source: AuthSource::None,
            },
        );
    }

    async fn get_or_create_provider_entry(&self, provider_id: ProviderId) -> Result<ProviderEntry> {
        // First check without holding the lock across await
        {
            let map = self.provider_map.read().unwrap();
            if let Some(entry) = map.get(&provider_id) {
                return Ok(entry.clone());
            }
        }

        // Get the provider config from registry
        let provider_config = self.provider_registry.get(&provider_id).ok_or_else(|| {
            crate::error::Error::Api(ApiError::Configuration(format!(
                "No provider configuration found for {provider_id:?}"
            )))
        })?;

        let resolved = self
            .config_provider
            .resolve_auth_for_provider(&provider_id)
            .await?;

        // Now acquire write lock and create provider
        let mut map = self.provider_map.write().unwrap();

        // Check again in case another thread added it
        if let Some(entry) = map.get(&provider_id) {
            return Ok(entry.clone());
        }

        let entry = self
            .build_provider_entry(provider_config, &resolved)
            .map_err(crate::error::Error::Api)?;

        map.insert(provider_id, entry.clone());
        Ok(entry)
    }

    fn build_provider_entry(
        &self,
        provider_config: &crate::config::provider::ProviderConfig,
        resolved: &ResolvedAuth,
    ) -> std::result::Result<ProviderEntry, ApiError> {
        let provider = match resolved {
            ResolvedAuth::Plugin { directive, .. } => {
                factory::create_provider_with_directive(provider_config, directive)?
            }
            ResolvedAuth::ApiKey { credential, .. } => {
                factory::create_provider(provider_config, credential)?
            }
            ResolvedAuth::None => {
                return Err(ApiError::Configuration(format!(
                    "No authentication configured for {:?}",
                    provider_config.id
                )));
            }
        };

        Ok(ProviderEntry {
            provider,
            auth_source: resolved.source(),
        })
    }

    async fn fallback_api_key_entry(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Option<ProviderEntry>> {
        let Some((key, origin)) = self
            .config_provider
            .resolve_api_key_for_provider(provider_id)
            .await?
        else {
            return Ok(None);
        };

        let provider_config = self.provider_registry.get(provider_id).ok_or_else(|| {
            crate::error::Error::Api(ApiError::Configuration(format!(
                "No provider configuration found for {provider_id:?}"
            )))
        })?;

        let credential = Credential::ApiKey { value: key };
        let provider = factory::create_provider(provider_config, &credential)
            .map_err(crate::error::Error::Api)?;

        Ok(Some(ProviderEntry {
            provider,
            auth_source: AuthSource::ApiKey { origin },
        }))
    }

    /// Complete a prompt with a specific model ID and optional parameters
    pub async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<crate::config::model::ModelParameters>,
        token: CancellationToken,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        // Get provider from model ID
        let provider_id = model_id.provider.clone();
        let entry = self
            .get_or_create_provider_entry(provider_id.clone())
            .await
            .map_err(ApiError::from)?;
        let provider = entry.provider.clone();

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
            .complete(
                model_id,
                messages.clone(),
                system.clone(),
                tools.clone(),
                effective_params,
                token.clone(),
            )
            .await;

        if let Err(ref err) = result {
            if Self::should_invalidate_provider(err) {
                self.invalidate_provider(&provider_id);

                if matches!(entry.auth_source, AuthSource::Plugin { .. }) {
                    if let Some(fallback) = self
                        .fallback_api_key_entry(&provider_id)
                        .await
                        .map_err(ApiError::from)?
                    {
                        let fallback_result = fallback
                            .provider
                            .complete(model_id, messages, system, tools, effective_params, token)
                            .await;
                        if fallback_result.is_ok() {
                            let mut map = self.provider_map.write().unwrap();
                            map.insert(provider_id, fallback);
                        }
                        return fallback_result;
                    }
                }
            }
        }

        result
    }

    pub async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<crate::config::model::ModelParameters>,
        token: CancellationToken,
    ) -> std::result::Result<CompletionStream, ApiError> {
        let provider_id = model_id.provider.clone();
        let entry = self
            .get_or_create_provider_entry(provider_id.clone())
            .await
            .map_err(ApiError::from)?;
        let provider = entry.provider.clone();

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
            .stream_complete(
                model_id,
                messages.clone(),
                system.clone(),
                tools.clone(),
                effective_params,
                token.clone(),
            )
            .await;

        if let Err(ref err) = result {
            if Self::should_invalidate_provider(err) {
                self.invalidate_provider(&provider_id);

                if matches!(entry.auth_source, AuthSource::Plugin { .. }) {
                    if let Some(fallback) = self
                        .fallback_api_key_entry(&provider_id)
                        .await
                        .map_err(ApiError::from)?
                    {
                        let fallback_result = fallback
                            .provider
                            .stream_complete(
                                model_id,
                                messages,
                                system,
                                tools,
                                effective_params,
                                token,
                            )
                            .await;
                        if fallback_result.is_ok() {
                            let mut map = self.provider_map.write().unwrap();
                            map.insert(provider_id, fallback);
                        }
                        return fallback_result;
                    }
                }
            }
        }

        result
    }

    pub async fn complete_with_retry(
        &self,
        model_id: &ModelId,
        messages: &[Message],
        system_prompt: &Option<SystemContext>,
        tools: &Option<Vec<ToolSchema>>,
        token: CancellationToken,
        max_attempts: usize,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        let mut attempts = 0;

        // Prepare provider and parameters once
        let provider_id = model_id.provider.clone();
        let entry = self
            .get_or_create_provider_entry(provider_id.clone())
            .await
            .map_err(ApiError::from)?;
        let provider = entry.provider.clone();

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
                        if matches!(entry.auth_source, AuthSource::Plugin { .. }) {
                            if let Some(fallback) = self
                                .fallback_api_key_entry(&provider_id)
                                .await
                                .map_err(ApiError::from)?
                            {
                                let fallback_result = fallback
                                    .provider
                                    .complete(
                                        model_id,
                                        messages.to_vec(),
                                        system_prompt.clone(),
                                        tools.clone(),
                                        effective_params,
                                        token.clone(),
                                    )
                                    .await;
                                if fallback_result.is_ok() {
                                    let mut map = self.provider_map.write().unwrap();
                                    map.insert(provider_id.clone(), fallback);
                                }
                                return fallback_result;
                            }
                        }
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
    use crate::auth::ApiKeyOrigin;
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
            _system: Option<SystemContext>,
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
        let config_provider = LlmConfigProvider::new(auth_storage).expect("config provider");
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));

        Client::new_with_deps(config_provider, provider_registry, model_registry)
    }

    fn insert_stub_provider(client: &Client, provider_id: ProviderId, error: StubErrorKind) {
        client.provider_map.write().unwrap().insert(
            provider_id,
            ProviderEntry {
                provider: Arc::new(StubProvider::new(error)),
                auth_source: AuthSource::ApiKey {
                    origin: ApiKeyOrigin::Stored,
                },
            },
        );
    }

    #[tokio::test]
    async fn invalidates_cached_provider_on_auth_failure() {
        let client = test_client();
        let provider_id = ProviderId("stub-auth".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");

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
        let model_id = ModelId::new(provider_id.clone(), "stub-model");

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
