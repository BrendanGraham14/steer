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
use crate::config::model::{ModelId, ModelParameters};
use crate::config::provider::ProviderId;
use crate::config::{LlmConfigProvider, ResolvedAuth};
use crate::error::Result;
use crate::model_registry::ModelRegistry;
pub use error::{ApiError, ProviderStreamErrorKind, SseParseError, StreamError};
pub use factory::{create_provider, create_provider_with_directive};
use futures::StreamExt;
use rand::Rng;
pub use provider::{CompletionResponse, CompletionStream, Provider, StreamChunk, TokenUsage};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;
use steer_tools::ToolSchema;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::app::SystemContext;
use crate::app::conversation::Message;

#[cfg(not(test))]
const RETRY_BASE_DELAY_MS: u64 = 250;
#[cfg(test)]
const RETRY_BASE_DELAY_MS: u64 = 1;
const RETRY_MAX_ATTEMPTS: usize = 5;

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
        let Ok(mut map) = self.provider_map.write() else {
            warn!(
                target: "api::client",
                "Provider cache lock poisoned while invalidating provider"
            );
            return;
        };
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

    /// Determine if an API error should trigger an automatic retry.
    fn should_retry_error(error: &ApiError) -> bool {
        match error {
            ApiError::Network(_) => true,
            ApiError::Timeout { .. } => true,
            ApiError::RateLimited { .. } => true,
            ApiError::ServerError { status_code, .. } => {
                matches!(status_code, 408 | 409 | 429 | 500 | 502 | 503 | 504)
            }
            _ => false,
        }
    }

    fn retry_delay(attempt: usize) -> Duration {
        let base_ms = RETRY_BASE_DELAY_MS * (1u64 << attempt.min(4));
        let jitter_percent = rand::thread_rng().gen_range(80_u64..=120_u64);
        let jittered_ms = base_ms.saturating_mul(jitter_percent).saturating_div(100).max(1);
        Duration::from_millis(jittered_ms)
    }

    fn should_retry_stream_error(error: &StreamError) -> bool {
        match error {
            StreamError::SseParse(SseParseError::Transport { .. }) => true,
            StreamError::Provider { kind, .. } => kind.is_retryable(),
            StreamError::Cancelled | StreamError::SseParse(_) => false,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Retry helper mirrors provider API inputs plus retry controls"
    )]
    async fn run_complete_with_retry(
        provider: &Arc<dyn Provider>,
        model_id: &ModelId,
        messages: &[Message],
        system: &Option<SystemContext>,
        tools: &Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: &CancellationToken,
        max_attempts: usize,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        let mut attempt = 0usize;

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
                    system.clone(),
                    tools.clone(),
                    call_options,
                    token.clone(),
                )
                .await
            {
                Ok(response) => return Ok(response),
                Err(error)
                    if Self::should_retry_error(&error)
                        && attempt + 1 < max_attempts
                        && !token.is_cancelled() =>
                {
                    attempt += 1;
                    let delay = Self::retry_delay(attempt - 1);
                    warn!(
                        target: "api::complete",
                        provider = provider.name(),
                        ?model_id,
                        attempt,
                        max_attempts,
                        ?delay,
                        error = %error,
                        "Retrying API completion after transient error"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Retry helper mirrors provider stream API inputs plus retry controls"
    )]
    async fn run_stream_start_with_retry(
        provider: &Arc<dyn Provider>,
        model_id: &ModelId,
        messages: &[Message],
        system: &Option<SystemContext>,
        tools: &Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: &CancellationToken,
        max_attempts: usize,
    ) -> std::result::Result<CompletionStream, ApiError> {
        let mut attempt = 0usize;

        loop {
            if token.is_cancelled() {
                return Err(ApiError::Cancelled {
                    provider: provider.name().to_string(),
                });
            }

            match provider
                .stream_complete(
                    model_id,
                    messages.to_vec(),
                    system.clone(),
                    tools.clone(),
                    call_options,
                    token.clone(),
                )
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(error)
                    if Self::should_retry_error(&error)
                        && attempt + 1 < max_attempts
                        && !token.is_cancelled() =>
                {
                    attempt += 1;
                    let delay = Self::retry_delay(attempt - 1);
                    warn!(
                        target: "api::stream_complete",
                        provider = provider.name(),
                        ?model_id,
                        attempt,
                        max_attempts,
                        ?delay,
                        error = %error,
                        "Retrying API stream initialization after transient error"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error),
            }
        }
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

    pub fn model_context_window_tokens(&self, model_id: &ModelId) -> Option<u32> {
        self.model_registry
            .get(model_id)
            .and_then(|model| model.context_window_tokens)
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn insert_test_provider(&self, provider_id: ProviderId, provider: Arc<dyn Provider>) {
        match self.provider_map.write() {
            Ok(mut map) => {
                map.insert(
                    provider_id,
                    ProviderEntry {
                        provider,
                        auth_source: AuthSource::None,
                    },
                );
            }
            Err(_) => {
                warn!(
                    target: "api::client",
                    "Provider cache lock poisoned while inserting test provider"
                );
            }
        }
    }

    async fn get_or_create_provider_entry(&self, provider_id: ProviderId) -> Result<ProviderEntry> {
        // First check without holding the lock across await
        {
            let map = self.provider_map.read().map_err(|_| {
                crate::error::Error::Api(ApiError::Configuration(
                    "Provider cache lock poisoned".to_string(),
                ))
            })?;
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
        let mut map = self.provider_map.write().map_err(|_| {
            crate::error::Error::Api(ApiError::Configuration(
                "Provider cache lock poisoned".to_string(),
            ))
        })?;

        // Check again in case another thread added it
        if let Some(entry) = map.get(&provider_id) {
            return Ok(entry.clone());
        }

        let entry = Self::build_provider_entry(provider_config, &resolved)?;

        map.insert(provider_id, entry.clone());
        Ok(entry)
    }

    fn build_provider_entry(
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
    ) -> std::result::Result<Option<ProviderEntry>, ApiError> {
        let Some((key, origin)) = self
            .config_provider
            .resolve_api_key_for_provider(provider_id)
            .await?
        else {
            return Ok(None);
        };

        let provider_config = self.provider_registry.get(provider_id).ok_or_else(|| {
            ApiError::Configuration(format!(
                "No provider configuration found for {provider_id:?}"
            ))
        })?;

        let credential = Credential::ApiKey { value: key };
        let provider = factory::create_provider(provider_config, &credential)?;

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

        let result = Self::run_complete_with_retry(
            &provider,
            model_id,
            &messages,
            &system,
            &tools,
            effective_params,
            &token,
            RETRY_MAX_ATTEMPTS,
        )
        .await;

        if let Err(ref err) = result
            && Self::should_invalidate_provider(err)
        {
            self.invalidate_provider(&provider_id);

            if matches!(entry.auth_source, AuthSource::Plugin { .. })
                && let Some(fallback) = self.fallback_api_key_entry(&provider_id).await?
            {
                let fallback_result = Self::run_complete_with_retry(
                    &fallback.provider,
                    model_id,
                    &messages,
                    &system,
                    &tools,
                    effective_params,
                    &token,
                    RETRY_MAX_ATTEMPTS,
                )
                .await;
                if fallback_result.is_ok() {
                    let mut map = self.provider_map.write().map_err(|_| {
                        ApiError::Configuration("Provider cache lock poisoned".to_string())
                    })?;
                    map.insert(provider_id, fallback);
                }
                return fallback_result;
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

        let (initial_stream, provider_for_retry) = match Self::run_stream_start_with_retry(
            &provider,
            model_id,
            &messages,
            &system,
            &tools,
            effective_params,
            &token,
            RETRY_MAX_ATTEMPTS,
        )
        .await
        {
            Ok(stream) => (stream, provider),
            Err(err) => {
                if Self::should_invalidate_provider(&err) {
                    self.invalidate_provider(&provider_id);

                    if matches!(entry.auth_source, AuthSource::Plugin { .. }) {
                        if let Some(fallback) = self.fallback_api_key_entry(&provider_id).await? {
                            let fallback_provider = fallback.provider.clone();
                            let fallback_stream = Self::run_stream_start_with_retry(
                                &fallback_provider,
                                model_id,
                                &messages,
                                &system,
                                &tools,
                                effective_params,
                                &token,
                                RETRY_MAX_ATTEMPTS,
                            )
                            .await?;
                            let mut map = self.provider_map.write().map_err(|_| {
                                ApiError::Configuration("Provider cache lock poisoned".to_string())
                            })?;
                            map.insert(provider_id, fallback);
                            (fallback_stream, fallback_provider)
                        } else {
                            return Err(err);
                        }
                    } else {
                        return Err(err);
                    }
                } else {
                    return Err(err);
                }
            }
        };

        let model_id = model_id.clone();
        let stream = async_stream::stream! {
            let mut attempt = 1usize;
            let mut current_stream = Some(initial_stream);

            'outer: loop {
                let mut saw_output = false;
                let mut stream = if let Some(stream) = current_stream.take() { stream } else {
                    if token.is_cancelled() {
                        yield StreamChunk::Error(StreamError::Cancelled);
                        break;
                    }

                    let stream_result = Self::run_stream_start_with_retry(
                        &provider_for_retry,
                        &model_id,
                        &messages,
                        &system,
                        &tools,
                        effective_params,
                        &token,
                        RETRY_MAX_ATTEMPTS,
                    )
                    .await;
                    match stream_result {
                        Ok(stream) => stream,
                        Err(err) => {
                            yield StreamChunk::Error(StreamError::Provider {
                                provider: provider_for_retry.name().to_string(),
                                kind: ProviderStreamErrorKind::StreamRetry,
                                raw_error_type: Some("stream_retry".to_string()),
                                message: err.to_string(),
                            });
                            break;
                        }
                    }
                };

                while let Some(chunk) = stream.next().await {
                    let retryable_stream_error = match &chunk {
                        StreamChunk::Error(stream_err) => match stream_err {
                            StreamError::Cancelled => false,
                            StreamError::SseParse(
                                SseParseError::Parser { .. } | SseParseError::Utf8 { .. },
                            ) => false,
                            _ => Self::should_retry_stream_error(stream_err),
                        },
                        _ => false,
                    };

                    if retryable_stream_error && attempt < RETRY_MAX_ATTEMPTS {
                        attempt += 1;
                        warn!(
                            target: "api::stream_complete",
                            ?model_id,
                            attempt,
                            max_attempts = RETRY_MAX_ATTEMPTS,
                            error = ?chunk,
                            "Retrying stream after transport/provider stream failure"
                        );
                        if saw_output {
                            yield StreamChunk::Reset;
                        }
                        current_stream = None;
                        continue 'outer;
                    }

                    if !matches!(chunk, StreamChunk::Error(_)) {
                        saw_output = true;
                    }

                    yield chunk;
                }

                break;
            }
        };

        Ok(Box::pin(stream))
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
        let provider_id = model_id.provider.clone();
        let entry = self
            .get_or_create_provider_entry(provider_id.clone())
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

        let result = Self::run_complete_with_retry(
            &entry.provider,
            model_id,
            messages,
            system_prompt,
            tools,
            effective_params,
            &token,
            max_attempts,
        )
        .await;

        if let Err(ref error) = result
            && Self::should_invalidate_provider(error)
        {
            self.invalidate_provider(&provider_id);
            if matches!(entry.auth_source, AuthSource::Plugin { .. })
                && let Some(fallback) = self.fallback_api_key_entry(&provider_id).await?
            {
                let fallback_result = Self::run_complete_with_retry(
                    &fallback.provider,
                    model_id,
                    messages,
                    system_prompt,
                    tools,
                    effective_params,
                    &token,
                    max_attempts,
                )
                .await;
                if fallback_result.is_ok() {
                    let mut map = self.provider_map.write().map_err(|_| {
                        ApiError::Configuration("Provider cache lock poisoned".to_string())
                    })?;
                    map.insert(provider_id, fallback);
                }
                return fallback_result;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::AssistantContent;
    use crate::auth::ApiKeyOrigin;
    use crate::config::provider::ProviderId;
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    #[derive(Clone)]
    struct FlakyCompleteProvider {
        failures_before_success: usize,
        attempts: Arc<AtomicUsize>,
    }

    impl FlakyCompleteProvider {
        fn new(failures_before_success: usize, attempts: Arc<AtomicUsize>) -> Self {
            Self {
                failures_before_success,
                attempts,
            }
        }
    }

    #[async_trait]
    impl Provider for FlakyCompleteProvider {
        fn name(&self) -> &'static str {
            "flaky-complete"
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
            let attempt = self.attempts.fetch_add(1, Ordering::Relaxed) + 1;
            if attempt <= self.failures_before_success {
                return Err(network_api_error());
            }
            Ok(success_response())
        }
    }

    #[derive(Clone)]
    struct FlakyStreamStartProvider {
        failures_before_success: usize,
        attempts: Arc<AtomicUsize>,
    }

    impl FlakyStreamStartProvider {
        fn new(failures_before_success: usize, attempts: Arc<AtomicUsize>) -> Self {
            Self {
                failures_before_success,
                attempts,
            }
        }
    }

    #[async_trait]
    impl Provider for FlakyStreamStartProvider {
        fn name(&self) -> &'static str {
            "flaky-stream-start"
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
            Ok(success_response())
        }

        async fn stream_complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> std::result::Result<CompletionStream, ApiError> {
            let attempt = self.attempts.fetch_add(1, Ordering::Relaxed) + 1;
            if attempt <= self.failures_before_success {
                return Err(network_api_error());
            }

            let response = success_response();
            Ok(Box::pin(futures_util::stream::once(async move {
                StreamChunk::MessageComplete(response)
            })))
        }
    }

    #[derive(Clone)]
    struct InvalidRequestProvider {
        attempts: Arc<AtomicUsize>,
    }

    impl InvalidRequestProvider {
        fn new(attempts: Arc<AtomicUsize>) -> Self {
            Self { attempts }
        }
    }

    #[async_trait]
    impl Provider for InvalidRequestProvider {
        fn name(&self) -> &'static str {
            "invalid-request"
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
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(ApiError::InvalidRequest {
                provider: "stub".to_string(),
                details: "bad request".to_string(),
            })
        }
    }

    fn success_response() -> CompletionResponse {
        CompletionResponse::new(vec![AssistantContent::Text {
            text: "ok".to_string(),
        }])
    }

    fn network_api_error() -> ApiError {
        let err = reqwest::Client::new()
            .get("http://[::1")
            .build()
            .expect_err("invalid URL should fail");
        ApiError::Network(err)
    }

    fn test_client() -> Client {
        let auth_storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        let config_provider = LlmConfigProvider::new(auth_storage).unwrap();
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));

        Client::new_with_deps(config_provider, provider_registry, model_registry)
    }

    fn insert_provider(client: &Client, provider_id: ProviderId, provider: Arc<dyn Provider>) {
        client.provider_map.write().unwrap().insert(
            provider_id,
            ProviderEntry {
                provider,
                auth_source: AuthSource::ApiKey {
                    origin: ApiKeyOrigin::Stored,
                },
            },
        );
    }

    fn insert_stub_provider(client: &Client, provider_id: ProviderId, error: StubErrorKind) {
        insert_provider(client, provider_id, Arc::new(StubProvider::new(error)));
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

    #[tokio::test]
    async fn retries_network_errors_for_complete() {
        let client = test_client();
        let provider_id = ProviderId("flaky-complete".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");
        let attempts = Arc::new(AtomicUsize::new(0));

        insert_provider(
            &client,
            provider_id,
            Arc::new(FlakyCompleteProvider::new(2, attempts.clone())),
        );

        let response = client
            .complete(
                &model_id,
                vec![],
                None,
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .expect("complete should retry transient network failures");

        assert_eq!(response.extract_text(), "ok");
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn does_not_retry_non_retryable_complete_error() {
        let client = test_client();
        let provider_id = ProviderId("invalid-request".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");
        let attempts = Arc::new(AtomicUsize::new(0));

        insert_provider(
            &client,
            provider_id,
            Arc::new(InvalidRequestProvider::new(attempts.clone())),
        );

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

        assert!(matches!(err, ApiError::InvalidRequest { .. }));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn retries_network_errors_when_starting_stream() {
        let client = test_client();
        let provider_id = ProviderId("flaky-stream-start".to_string());
        let model_id = ModelId::new(provider_id.clone(), "stub-model");
        let attempts = Arc::new(AtomicUsize::new(0));

        insert_provider(
            &client,
            provider_id,
            Arc::new(FlakyStreamStartProvider::new(2, attempts.clone())),
        );

        let mut stream = client
            .stream_complete(
                &model_id,
                vec![],
                None,
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .expect("stream start should retry transient network failures");

        let chunk = stream.next().await.expect("stream should yield completion");
        match chunk {
            StreamChunk::MessageComplete(response) => assert_eq!(response.extract_text(), "ok"),
            other => panic!("unexpected stream chunk: {other:?}"),
        }

        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }
}
