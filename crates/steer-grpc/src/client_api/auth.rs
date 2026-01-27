//! Client-facing authentication types and helpers.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    OAuth,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    ApiKey { origin: ApiKeyOrigin },
    Plugin { method: AuthMethod },
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyOrigin {
    Env,
    Stored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthProgress {
    NeedInput { prompt: String },
    InProgress { message: String },
    Complete,
    Error { message: String },
    OAuthStarted { auth_url: String },
}

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ProviderAuthStatus {
    pub provider_id: String,
    pub auth_source: Option<AuthSource>,
}

#[derive(Debug, Clone)]
pub struct StartAuthResponse {
    pub flow_id: String,
    pub progress: Option<AuthProgress>,
}
