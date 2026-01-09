use crate::api::error::ApiError;
use crate::api::openai::responses;
use crate::api::openai::responses_types::{ExtraValue, ResponsesRequest};
use crate::api::provider::{CompletionResponse, CompletionStream, Provider};
use crate::auth::openai::{extract_chatgpt_account_id, refresh_if_needed, OpenAIOAuth};
use crate::auth::AuthStorage;
use crate::config::model::{ModelId, ModelParameters};
use async_trait::async_trait;
use reqwest::header;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_BETA: &str = "responses=experimental";
const ORIGINATOR: &str = "codex_cli_rs";
const CODEX_BASE_INSTRUCTIONS: &str =
    include_str!("../../../assets/codex/gpt-5.2-codex_prompt.md");

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
        let system_prompt = system
            .as_deref()
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .map(str::to_owned);
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
        request.instructions = Some(
            system_prompt.unwrap_or_else(|| CODEX_BASE_INSTRUCTIONS.to_string()),
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

        let id_token = tokens.id_token.as_deref().ok_or_else(|| {
            ApiError::AuthenticationFailed {
                provider: "openai".to_string(),
                details: "Missing id_token in OAuth credentials".to_string(),
            }
        })?;

        let account_id = extract_chatgpt_account_id(id_token)
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

        responses::stream_responses_request(request_builder, token).await
    }
}
