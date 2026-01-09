use crate::api::error::ApiError;
use crate::api::openai::responses;
use crate::api::openai::responses_types::{ExtraValue, ResponsesRequest};
use crate::api::provider::{CompletionResponse, CompletionStream, Provider};
use crate::api::sse::parse_sse_stream;
use crate::auth::openai::{refresh_if_needed, resolve_chatgpt_account_id, OpenAIOAuth};
use crate::auth::AuthStorage;
use crate::config::model::{ModelId, ModelParameters};
use async_trait::async_trait;
use reqwest::header;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_BETA: &str = "responses=experimental";
const ORIGINATOR: &str = "codex_cli_rs";

pub struct CodexClient {
    http_client: reqwest::Client,
    base_url: String,
    responses_client: responses::Client,
    storage: Arc<dyn AuthStorage>,
    oauth_client: OpenAIOAuth,
}

impl CodexClient {
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            header::HeaderName::from_static("openai-beta"),
            header::HeaderValue::from_static(OPENAI_BETA),
        );
        headers.insert(
            header::HeaderName::from_static("originator"),
            header::HeaderValue::from_static(ORIGINATOR),
        );

        let http_client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(super::HTTP_TIMEOUT_SECS))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http_client,
            base_url: CODEX_BASE_URL.to_string(),
            responses_client: responses::Client::new("unused".to_string()),
            storage,
            oauth_client: OpenAIOAuth::new(),
        }
    }

    fn build_request(
        &self,
        model_id: &ModelId,
        messages: Vec<crate::app::conversation::Message>,
        system: Option<String>,
        tools: Option<Vec<steer_tools::ToolSchema>>,
        call_options: Option<ModelParameters>,
    ) -> ResponsesRequest {
        let mut request = self
            .responses_client
            .build_request(model_id, messages, system, tools, call_options);
        request.store = Some(false);
        request.extra.insert(
            "include".to_string(),
            ExtraValue::Array(vec![ExtraValue::String(
                "reasoning.encrypted_content".to_string(),
            )]),
        );
        request
    }

    async fn oauth_headers(&self) -> Result<header::HeaderMap, ApiError> {
        let tokens = refresh_if_needed(&self.storage, &self.oauth_client)
            .await
            .map_err(|e| ApiError::AuthenticationFailed {
                provider: "openai".to_string(),
                details: e.to_string(),
            })?;

        let account_id = resolve_chatgpt_account_id(&self.oauth_client, &tokens.access_token)
            .await
            .map_err(|e| ApiError::AuthenticationFailed {
                provider: "openai".to_string(),
                details: e.to_string(),
            })?;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", tokens.access_token))
                .map_err(|e| ApiError::AuthenticationFailed {
                    provider: "openai".to_string(),
                    details: format!("Invalid access token: {e}"),
                })?,
        );
        headers.insert(
            header::HeaderName::from_static("chatgpt-account-id"),
            header::HeaderValue::from_str(&account_id.0).map_err(|e| {
                ApiError::AuthenticationFailed {
                    provider: "openai".to_string(),
                    details: format!("Invalid account id: {e}"),
                }
            })?,
        );

        Ok(headers)
    }
}

#[async_trait]
impl Provider for CodexClient {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn complete(
        &self,
        _model_id: &ModelId,
        _messages: Vec<crate::app::conversation::Message>,
        _system: Option<String>,
        _tools: Option<Vec<steer_tools::ToolSchema>>,
        _call_options: Option<ModelParameters>,
        _token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        Err(ApiError::Configuration(
            "OpenAI OAuth requests require streaming responses".to_string(),
        ))
    }

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<crate::app::conversation::Message>,
        system: Option<String>,
        tools: Option<Vec<steer_tools::ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let mut request = self.build_request(model_id, messages, system, tools, call_options);
        request.stream = Some(true);

        let auth_headers = self.oauth_headers().await?;

        let request_builder = self
            .http_client
            .post(&self.base_url)
            .headers(auth_headers)
            .json(&request);

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "openai::codex", "Cancellation token triggered before sending request.");
                return Err(ApiError::Cancelled{ provider: "openai".to_string()});
            }
            res = request_builder.send() => {
                res.map_err(|e| {
                    error!(target: "openai::codex", "Request send failed: {}", e);
                    ApiError::Network(e)
                })?
            }
        };

        if token.is_cancelled() {
            debug!(target: "openai::codex", "Cancellation token triggered after sending request.");
            return Err(ApiError::Cancelled {
                provider: "openai".to_string(),
            });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(
                target: "openai::codex",
                "Request failed with status {}: {}", status, body
            );
            return Err(ApiError::ServerError {
                provider: "openai".to_string(),
                status_code: status.as_u16(),
                details: body,
            });
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(responses::Client::convert_responses_stream(
            sse_stream, token,
        )))
    }
}
