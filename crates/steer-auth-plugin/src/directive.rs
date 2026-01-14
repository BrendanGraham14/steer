use crate::error::Result;
use crate::identifiers::ModelId;
use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct QueryParam {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy)]
pub enum RequestKind {
    Complete,
    Stream,
}

#[derive(Debug, Clone)]
pub struct AuthHeaderContext {
    pub model_id: Option<ModelId>,
    pub request_kind: RequestKind,
}

#[derive(Debug, Clone)]
pub struct AuthErrorContext {
    pub status: Option<u16>,
    pub body_snippet: Option<String>,
    pub request_kind: RequestKind,
}

#[derive(Debug, Clone, Copy)]
pub enum AuthErrorAction {
    RetryOnce,
    ReauthRequired,
    NoAction,
}

#[async_trait]
pub trait AuthHeaderProvider: Send + Sync {
    async fn headers(&self, ctx: AuthHeaderContext) -> Result<Vec<HeaderPair>>;

    async fn on_auth_error(&self, ctx: AuthErrorContext) -> Result<AuthErrorAction>;
}

#[derive(Debug, Clone)]
pub enum InstructionPolicy {
    Prefix(String),
    DefaultIfEmpty(String),
    Override(String),
}

#[derive(Debug, Clone)]
pub enum AuthDirective {
    OpenAiResponses(OpenAiResponsesAuth),
    Anthropic(AnthropicAuth),
}

#[derive(Clone)]
pub struct OpenAiResponsesAuth {
    pub headers: Arc<dyn AuthHeaderProvider>,
    pub base_url_override: Option<String>,
    pub require_streaming: Option<bool>,
    pub instruction_policy: Option<InstructionPolicy>,
    pub include: Option<Vec<String>>,
}

impl fmt::Debug for OpenAiResponsesAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiResponsesAuth")
            .field("headers", &"<AuthHeaderProvider>")
            .field("base_url_override", &self.base_url_override)
            .field("require_streaming", &self.require_streaming)
            .field("instruction_policy", &self.instruction_policy)
            .field("include", &self.include)
            .finish()
    }
}

#[derive(Clone)]
pub struct AnthropicAuth {
    pub headers: Arc<dyn AuthHeaderProvider>,
    pub instruction_policy: Option<InstructionPolicy>,
    pub query_params: Option<Vec<QueryParam>>,
}

impl fmt::Debug for AnthropicAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicAuth")
            .field("headers", &"<AuthHeaderProvider>")
            .field("instruction_policy", &self.instruction_policy)
            .field("query_params", &self.query_params)
            .finish()
    }
}
