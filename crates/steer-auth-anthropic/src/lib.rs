use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use steer_auth_plugin::AuthPlugin;
use steer_auth_plugin::{
    AnthropicAuth, AuthDirective, AuthError, AuthErrorAction, AuthErrorContext, AuthHeaderContext,
    AuthHeaderProvider, AuthMethod, AuthProgress, AuthStorage, AuthTokens, AuthenticationFlow,
    Credential, CredentialType, DynAuthenticationFlow, HeaderPair, InstructionPolicy, ProviderId,
    QueryParam, Result,
};

const PROVIDER_ID: &str = "anthropic";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

#[derive(Debug)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Clone)]
pub struct AnthropicOAuth {
    client_id: String,
    redirect_uri: String,
    http_client: reqwest::Client,
}

impl Default for AnthropicOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicOAuth {
    pub fn new() -> Self {
        Self {
            client_id: CLIENT_ID.to_string(),
            redirect_uri: REDIRECT_URI.to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    /// Generate PKCE challenge
    pub fn generate_pkce() -> PkceChallenge {
        let verifier = generate_random_string(128);
        let challenge = base64_url_encode(&sha256(&verifier));
        PkceChallenge {
            verifier,
            challenge,
        }
    }

    /// Build authorization URL
    pub fn build_auth_url(&self, pkce: &PkceChallenge) -> String {
        let params = [
            ("code", "true"),
            ("client_id", &self.client_id),
            ("response_type", "code"),
            ("redirect_uri", &self.redirect_uri),
            ("scope", SCOPES),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", &pkce.verifier),
        ];

        let query = serde_urlencoded::to_string(params).unwrap_or_default();
        format!("{AUTHORIZE_URL}?{query}")
    }

    /// Parse the callback code from the redirect URL or query fragment.
    pub fn parse_callback_code(callback_code: &str) -> Result<(String, String)> {
        let trimmed = callback_code.trim();
        if trimmed.is_empty() {
            return Err(AuthError::InvalidResponse(
                "Invalid callback code format. Expected a URL or code/state parameters."
                    .to_string(),
            ));
        }

        if let Ok(url) = reqwest::Url::parse(trimmed)
            && let Some(pair) = extract_code_state_from_url(&url)
        {
            return Ok(pair);
        }

        if let Some(pair) = extract_code_state_from_str(trimmed) {
            return Ok(pair);
        }

        if let Some(pair) = extract_legacy_code_state(trimmed) {
            return Ok(pair);
        }

        Err(AuthError::InvalidResponse(
            "Invalid callback code format. Expected a URL or code/state parameters.".to_string(),
        ))
    }

    /// Exchange authorization code for tokens
    pub async fn exchange_code_for_tokens(
        &self,
        code: &str,
        state: &str,
        pkce_verifier: &str,
    ) -> Result<AuthTokens> {
        #[derive(Serialize)]
        struct TokenRequest {
            code: String,
            state: String,
            grant_type: String,
            client_id: String,
            redirect_uri: String,
            code_verifier: String,
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: u64,
        }

        let request = TokenRequest {
            code: code.to_string(),
            state: state.to_string(),
            grant_type: "authorization_code".to_string(),
            client_id: self.client_id.clone(),
            redirect_uri: self.redirect_uri.clone(),
            code_verifier: pkce_verifier.to_string(),
        };

        let response = self
            .http_client
            .post(TOKEN_URL)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AuthError::InvalidResponse(format!(
                "Token exchange failed with status {status}: {error_text}"
            )));
        }

        let token_response: TokenResponse = response.json().await.map_err(|e| {
            AuthError::InvalidResponse(format!("Failed to parse token response: {e}"))
        })?;

        let expires_at = SystemTime::now() + Duration::from_secs(token_response.expires_in);

        Ok(AuthTokens {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at,
            id_token: None,
        })
    }

    /// Refresh access token using refresh token
    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<AuthTokens> {
        #[derive(Serialize)]
        struct RefreshRequest {
            grant_type: String,
            refresh_token: String,
            client_id: String,
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: u64,
        }

        let request = RefreshRequest {
            grant_type: "refresh_token".to_string(),
            refresh_token: refresh_token.to_string(),
            client_id: self.client_id.clone(),
        };

        let response = self
            .http_client
            .post(TOKEN_URL)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(AuthError::ReauthRequired);
            }

            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AuthError::InvalidResponse(format!(
                "Token refresh failed with status {status}: {error_text}"
            )));
        }

        let token_response: TokenResponse = response.json().await.map_err(|e| {
            AuthError::InvalidResponse(format!("Failed to parse refresh response: {e}"))
        })?;

        let expires_at = SystemTime::now() + Duration::from_secs(token_response.expires_in);

        Ok(AuthTokens {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at,
            id_token: None,
        })
    }
}

fn resolve_callback_input(input: &str, verifier: &str) -> Result<(String, String)> {
    match AnthropicOAuth::parse_callback_code(input) {
        Ok(pair) => Ok(pair),
        Err(err) => {
            let trimmed = input.trim();
            let fallback_code = extract_code_only_from_str(trimmed).or_else(|| {
                reqwest::Url::parse(trimmed)
                    .ok()
                    .and_then(|url| extract_code_only_from_url(&url))
            });

            if let Some(code) = fallback_code {
                Ok((code, verifier.to_string()))
            } else {
                Err(err)
            }
        }
    }
}

fn extract_code_state_from_url(url: &reqwest::Url) -> Option<(String, String)> {
    if let Some(query) = url.query()
        && let Some(pair) = extract_code_state_from_kv(query)
    {
        return Some(pair);
    }

    if let Some(fragment) = url.fragment()
        && let Some(pair) = extract_code_state_from_kv(fragment)
    {
        return Some(pair);
    }

    None
}

fn extract_code_state_from_str(input: &str) -> Option<(String, String)> {
    if let Some(pair) = extract_code_state_from_kv(input) {
        return Some(pair);
    }

    if let Some(query_start) = input.find('?')
        && let Some(pair) = extract_code_state_from_kv(&input[query_start + 1..])
    {
        return Some(pair);
    }

    if let Some(fragment_start) = input.find('#')
        && let Some(pair) = extract_code_state_from_kv(&input[fragment_start + 1..])
    {
        return Some(pair);
    }

    None
}

fn extract_code_state_from_kv(raw: &str) -> Option<(String, String)> {
    if raw.is_empty() {
        return None;
    }

    let params: HashMap<String, String> = serde_urlencoded::from_str(raw).ok()?;
    let code = params.get("code")?;
    let state = params.get("state")?;
    Some((code.clone(), state.clone()))
}

fn extract_code_only_from_url(url: &reqwest::Url) -> Option<String> {
    if let Some(query) = url.query()
        && let Some(code) = extract_code_only_from_kv(query)
    {
        return Some(code);
    }

    if let Some(fragment) = url.fragment()
        && let Some(code) = extract_code_only_from_kv(fragment)
    {
        return Some(code);
    }

    None
}

fn extract_code_only_from_str(input: &str) -> Option<String> {
    if let Some(code) = extract_code_only_from_kv(input) {
        return Some(code);
    }

    if let Some(query_start) = input.find('?')
        && let Some(code) = extract_code_only_from_kv(&input[query_start + 1..])
    {
        return Some(code);
    }

    if let Some(fragment_start) = input.find('#')
        && let Some(code) = extract_code_only_from_kv(&input[fragment_start + 1..])
    {
        return Some(code);
    }

    None
}

fn extract_code_only_from_kv(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }

    let params: HashMap<String, String> = serde_urlencoded::from_str(raw).ok()?;
    params.get("code").cloned()
}

fn extract_legacy_code_state(input: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = input.split('#').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

/// Check if tokens need refresh (within 5 minutes of expiry)
pub fn tokens_need_refresh(tokens: &AuthTokens) -> bool {
    match tokens.expires_at.duration_since(SystemTime::now()) {
        Ok(duration) => duration.as_secs() <= 300,
        Err(_) => true,
    }
}

/// Get OAuth headers for Anthropic API requests
pub fn get_oauth_headers(access_token: &str) -> Vec<HeaderPair> {
    vec![
        HeaderPair {
            name: "authorization".to_string(),
            value: format!("Bearer {access_token}"),
        },
        HeaderPair {
            name: "anthropic-beta".to_string(),
            value: "oauth-2025-04-20,interleaved-thinking-2025-05-14,claude-code-20250219"
                .to_string(),
        },
        HeaderPair {
            name: "user-agent".to_string(),
            value: "claude-cli/2.1.2 (external, cli)".to_string(),
        },
    ]
}

/// Helper to refresh tokens if needed
pub async fn refresh_if_needed(
    storage: &Arc<dyn AuthStorage>,
    oauth_client: &AnthropicOAuth,
) -> Result<AuthTokens> {
    let credential = storage
        .get_credential(PROVIDER_ID, CredentialType::OAuth2)
        .await?
        .ok_or(AuthError::ReauthRequired)?;

    let mut tokens = match credential {
        Credential::OAuth2(tokens) => tokens,
        _ => return Err(AuthError::ReauthRequired),
    };

    if tokens_need_refresh(&tokens) {
        match oauth_client.refresh_tokens(&tokens.refresh_token).await {
            Ok(new_tokens) => {
                storage
                    .set_credential(PROVIDER_ID, Credential::OAuth2(new_tokens.clone()))
                    .await?;
                tokens = new_tokens;
            }
            Err(AuthError::ReauthRequired) => {
                storage
                    .remove_credential(PROVIDER_ID, CredentialType::OAuth2)
                    .await?;
                return Err(AuthError::ReauthRequired);
            }
            Err(e) => return Err(e),
        }
    }

    Ok(tokens)
}

async fn force_refresh(
    storage: &Arc<dyn AuthStorage>,
    oauth_client: &AnthropicOAuth,
) -> Result<AuthTokens> {
    let credential = storage
        .get_credential(PROVIDER_ID, CredentialType::OAuth2)
        .await?
        .ok_or(AuthError::ReauthRequired)?;

    let tokens = match credential {
        Credential::OAuth2(tokens) => tokens,
        _ => return Err(AuthError::ReauthRequired),
    };

    match oauth_client.refresh_tokens(&tokens.refresh_token).await {
        Ok(new_tokens) => {
            storage
                .set_credential(PROVIDER_ID, Credential::OAuth2(new_tokens.clone()))
                .await?;
            Ok(new_tokens)
        }
        Err(AuthError::ReauthRequired) => {
            storage
                .remove_credential(PROVIDER_ID, CredentialType::OAuth2)
                .await?;
            Err(AuthError::ReauthRequired)
        }
        Err(err) => Err(err),
    }
}

fn generate_random_string(length: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();

    (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn sha256(data: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hasher.finalize().to_vec()
}

fn base64_url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

#[derive(Debug, Clone)]
pub struct AnthropicAuthState {
    pub kind: AnthropicAuthStateKind,
}

#[derive(Debug, Clone)]
pub enum AnthropicAuthStateKind {
    OAuthStarted { verifier: String, auth_url: String },
}

pub struct AnthropicOAuthFlow {
    storage: Arc<dyn AuthStorage>,
    oauth_client: AnthropicOAuth,
}

impl AnthropicOAuthFlow {
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self {
            storage,
            oauth_client: AnthropicOAuth::new(),
        }
    }
}

#[async_trait]
impl AuthenticationFlow for AnthropicOAuthFlow {
    type State = AnthropicAuthState;

    fn available_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::OAuth]
    }

    async fn start_auth(&self, method: AuthMethod) -> Result<Self::State> {
        match method {
            AuthMethod::OAuth => {
                let pkce = AnthropicOAuth::generate_pkce();
                let auth_url = self.oauth_client.build_auth_url(&pkce);

                Ok(AnthropicAuthState {
                    kind: AnthropicAuthStateKind::OAuthStarted {
                        verifier: pkce.verifier,
                        auth_url,
                    },
                })
            }
            _ => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: PROVIDER_ID.to_string(),
            }),
        }
    }

    async fn get_initial_progress(
        &self,
        state: &Self::State,
        method: AuthMethod,
    ) -> Result<AuthProgress> {
        match method {
            AuthMethod::OAuth => {
                let AnthropicAuthStateKind::OAuthStarted { auth_url, .. } = &state.kind;
                Ok(AuthProgress::OAuthStarted {
                    auth_url: auth_url.clone(),
                })
            }
            _ => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: PROVIDER_ID.to_string(),
            }),
        }
    }

    async fn handle_input(&self, state: &mut Self::State, input: &str) -> Result<AuthProgress> {
        match &mut state.kind {
            AnthropicAuthStateKind::OAuthStarted { verifier, .. } => {
                if input.trim().is_empty() {
                    return Ok(AuthProgress::NeedInput(
                        "Paste the redirect URL or code from your browser".to_string(),
                    ));
                }

                let (code, state_param) = resolve_callback_input(input, verifier)?;

                let tokens = self
                    .oauth_client
                    .exchange_code_for_tokens(&code, &state_param, verifier)
                    .await?;

                self.storage
                    .set_credential(PROVIDER_ID, Credential::OAuth2(tokens))
                    .await?;

                Ok(AuthProgress::Complete)
            }
        }
    }

    async fn is_authenticated(&self) -> Result<bool> {
        if let Some(Credential::OAuth2(tokens)) = self
            .storage
            .get_credential(PROVIDER_ID, CredentialType::OAuth2)
            .await?
        {
            return Ok(!tokens_need_refresh(&tokens));
        }

        Ok(false)
    }

    fn provider_name(&self) -> String {
        PROVIDER_ID.to_string()
    }
}

#[derive(Clone)]
struct AnthropicHeaderProvider {
    storage: Arc<dyn AuthStorage>,
    oauth: AnthropicOAuth,
}

impl AnthropicHeaderProvider {
    fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self {
            storage,
            oauth: AnthropicOAuth::new(),
        }
    }

    async fn header_pairs(&self, _ctx: AuthHeaderContext) -> Result<Vec<HeaderPair>> {
        let tokens = refresh_if_needed(&self.storage, &self.oauth).await?;
        Ok(get_oauth_headers(&tokens.access_token))
    }
}

#[async_trait]
impl AuthHeaderProvider for AnthropicHeaderProvider {
    async fn headers(&self, ctx: AuthHeaderContext) -> Result<Vec<HeaderPair>> {
        self.header_pairs(ctx).await
    }

    async fn on_auth_error(&self, _ctx: AuthErrorContext) -> Result<AuthErrorAction> {
        match force_refresh(&self.storage, &self.oauth).await {
            Ok(_) => Ok(AuthErrorAction::RetryOnce),
            Err(AuthError::ReauthRequired) => Ok(AuthErrorAction::ReauthRequired),
            Err(err) => Err(err),
        }
    }
}

#[derive(Clone)]
pub struct AnthropicAuthPlugin;

impl Default for AnthropicAuthPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicAuthPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AuthPlugin for AnthropicAuthPlugin {
    fn provider_id(&self) -> ProviderId {
        ProviderId(PROVIDER_ID.to_string())
    }

    fn supported_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::OAuth]
    }

    fn create_flow(&self, storage: Arc<dyn AuthStorage>) -> Option<Box<dyn DynAuthenticationFlow>> {
        Some(Box::new(steer_auth_plugin::AuthFlowWrapper::new(
            AnthropicOAuthFlow::new(storage),
        )))
    }

    async fn resolve_auth(&self, storage: Arc<dyn AuthStorage>) -> Result<Option<AuthDirective>> {
        let is_authenticated = self.is_authenticated(storage.clone()).await?;
        if !is_authenticated {
            return Ok(None);
        }

        let headers = Arc::new(AnthropicHeaderProvider::new(storage));
        let directive = AnthropicAuth {
            headers,
            instruction_policy: Some(InstructionPolicy::Prefix(
                "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
            )),
            query_params: Some(vec![QueryParam {
                name: "beta".to_string(),
                value: "true".to_string(),
            }]),
        };

        Ok(Some(AuthDirective::Anthropic(directive)))
    }

    async fn is_authenticated(&self, storage: Arc<dyn AuthStorage>) -> Result<bool> {
        if let Some(Credential::OAuth2(tokens)) = storage
            .get_credential(PROVIDER_ID, CredentialType::OAuth2)
            .await?
        {
            return Ok(!tokens_need_refresh(&tokens));
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    struct TestAuthStorage {
        credentials: Arc<Mutex<HashMap<String, Credential>>>,
    }

    impl TestAuthStorage {
        fn new() -> Self {
            Self {
                credentials: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl AuthStorage for TestAuthStorage {
        async fn get_credential(
            &self,
            provider: &str,
            credential_type: CredentialType,
        ) -> Result<Option<Credential>> {
            let key = format!("{provider}-{credential_type}");
            Ok(self.credentials.lock().await.get(&key).cloned())
        }

        async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
            let key = format!("{}-{}", provider, credential.credential_type());
            self.credentials.lock().await.insert(key, credential);
            Ok(())
        }

        async fn remove_credential(
            &self,
            provider: &str,
            credential_type: CredentialType,
        ) -> Result<()> {
            let key = format!("{provider}-{credential_type}");
            self.credentials.lock().await.remove(&key);
            Ok(())
        }
    }

    #[test]
    fn test_pkce_generation() {
        let pkce = AnthropicOAuth::generate_pkce();

        assert_eq!(pkce.verifier.len(), 128);
        assert_eq!(pkce.challenge.len(), 43);

        let expected_challenge = base64_url_encode(&sha256(&pkce.verifier));
        assert_eq!(pkce.challenge, expected_challenge);
    }

    #[test]
    fn test_state_generation() {
        let pkce1 = AnthropicOAuth::generate_pkce();
        let pkce2 = AnthropicOAuth::generate_pkce();

        assert_ne!(pkce1.verifier, pkce2.verifier);
    }

    #[test]
    fn test_build_auth_url() {
        let oauth = AnthropicOAuth::new();
        let pkce = AnthropicOAuth::generate_pkce();
        let url = oauth.build_auth_url(&pkce);

        assert!(url.contains(AUTHORIZE_URL));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(
            "redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"
        ));
    }

    #[test]
    fn test_parse_callback_code_from_url() {
        let input = "https://console.anthropic.com/oauth/code/callback?code=abc123&state=state456";
        let (code, state) = AnthropicOAuth::parse_callback_code(input).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "state456");
    }

    #[test]
    fn test_parse_callback_code_from_query() {
        let input = "code=abc123&state=state456";
        let (code, state) = AnthropicOAuth::parse_callback_code(input).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "state456");
    }

    #[test]
    fn test_parse_callback_code_from_fragment() {
        let input = "https://console.anthropic.com/oauth/code/callback#code=abc123&state=state456";
        let (code, state) = AnthropicOAuth::parse_callback_code(input).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "state456");
    }

    #[test]
    fn test_parse_callback_code_legacy() {
        let input = "abc123#state456";
        let (code, state) = AnthropicOAuth::parse_callback_code(input).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "state456");
    }

    #[test]
    fn test_extract_code_only_from_query() {
        let input = "code=abc123";
        let code = extract_code_only_from_str(input).unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn test_extract_code_only_from_url() {
        let input = "https://console.anthropic.com/oauth/code/callback?code=abc123";
        let code = extract_code_only_from_str(input).unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn test_extract_code_only_from_fragment() {
        let input = "https://console.anthropic.com/oauth/code/callback#code=abc123";
        let code = extract_code_only_from_str(input).unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn test_resolve_callback_input_code_only_uses_verifier() {
        let (code, state) = resolve_callback_input("code=abc123", "verifier-123").unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "verifier-123");
    }

    #[tokio::test]
    async fn test_handle_input_empty_returns_need_input() {
        let storage = Arc::new(TestAuthStorage::new());
        let flow = AnthropicOAuthFlow::new(storage);
        let mut state = flow.start_auth(AuthMethod::OAuth).await.unwrap();

        let progress = flow.handle_input(&mut state, "").await.unwrap();

        match progress {
            AuthProgress::NeedInput(message) => {
                assert!(message.contains("Paste the redirect URL"));
            }
            other => panic!("Expected NeedInput, got {other:?}"),
        }
    }

    #[test]
    fn test_get_oauth_headers() {
        let headers = get_oauth_headers("test-token");

        assert_eq!(headers.len(), 3);

        let auth = headers.iter().find(|h| h.name == "authorization").unwrap();
        assert_eq!(auth.value, "Bearer test-token");

        let beta = headers.iter().find(|h| h.name == "anthropic-beta").unwrap();
        assert!(beta.value.contains("oauth-2025-04-20"));
        assert!(beta.value.contains("interleaved-thinking-2025-05-14"));
        assert!(beta.value.contains("claude-code-20250219"));

        let ua = headers.iter().find(|h| h.name == "user-agent").unwrap();
        assert_eq!(ua.value, "claude-cli/2.1.2 (external, cli)");
    }
}
