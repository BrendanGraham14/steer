use crate::auth::callback_server::{spawn_callback_server, CallbackResponse, CallbackServerHandle};
use crate::auth::{AuthError, AuthStorage, AuthTokens, Credential, CredentialType, Result};
use crate::auth::{AuthMethod, AuthProgress, AuthenticationFlow};
use crate::config::provider;
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

// OAuth constants
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPES: &str = "openid profile email offline_access";
const ORIGINATOR: &str = "codex_cli_rs";
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_PORT: u16 = 1455;

const CHATGPT_ACCOUNT_ID_NESTED_CLAIM: &str = "https://api.openai.com/auth";

#[derive(Debug)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatGptAccountId(pub String);

pub struct OpenAIOAuth {
    client_id: String,
    redirect_uri: String,
    http_client: reqwest::Client,
}

impl Default for OpenAIOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAIOAuth {
    pub fn new() -> Self {
        Self {
            client_id: CLIENT_ID.to_string(),
            redirect_uri: REDIRECT_URI.to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    pub fn generate_pkce() -> PkceChallenge {
        let verifier = generate_random_string(128);
        let challenge = base64_url_encode(&sha256(&verifier));
        PkceChallenge {
            verifier,
            challenge,
        }
    }

    pub fn generate_state() -> String {
        generate_random_string(32)
    }

    pub fn build_auth_url(&self, pkce: &PkceChallenge, state: &str) -> String {
        let params = [
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", &self.redirect_uri),
            ("scope", SCOPES),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", ORIGINATOR),
        ];

        let query = serde_urlencoded::to_string(params).unwrap_or_default();
        format!("{AUTHORIZE_URL}?{query}")
    }

    pub async fn exchange_code_for_tokens(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<AuthTokens> {
        #[derive(Serialize)]
        struct TokenRequest {
            grant_type: String,
            client_id: String,
            code: String,
            redirect_uri: String,
            code_verifier: String,
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            id_token: Option<String>,
            access_token: String,
            refresh_token: Option<String>,
            expires_in: Option<u64>,
        }

        let request = TokenRequest {
            grant_type: "authorization_code".to_string(),
            client_id: self.client_id.clone(),
            code: code.to_string(),
            redirect_uri: self.redirect_uri.clone(),
            code_verifier: pkce_verifier.to_string(),
        };

        let response = self
            .http_client
            .post(TOKEN_URL)
            .form(&request)
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

        if token_response.access_token.trim().is_empty() {
            return Err(AuthError::InvalidResponse(
                "Empty access_token in token response".to_string(),
            ));
        }

        let id_token = token_response.id_token.ok_or_else(|| {
            AuthError::InvalidResponse("Missing id_token in token response".to_string())
        })?;

        let refresh_token = token_response.refresh_token.ok_or_else(|| {
            AuthError::InvalidResponse("Missing refresh_token in token response".to_string())
        })?;

        let expires_at =
            resolve_expires_at(token_response.expires_in, &token_response.access_token)?;

        Ok(AuthTokens {
            access_token: token_response.access_token,
            refresh_token,
            expires_at,
            id_token: Some(id_token),
        })
    }

    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<AuthTokens> {
        #[derive(Serialize)]
        struct RefreshRequest {
            grant_type: String,
            refresh_token: String,
            client_id: String,
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            id_token: Option<String>,
            access_token: String,
            refresh_token: Option<String>,
            expires_in: Option<u64>,
        }

        let request = RefreshRequest {
            grant_type: "refresh_token".to_string(),
            refresh_token: refresh_token.to_string(),
            client_id: self.client_id.clone(),
        };

        let response = self
            .http_client
            .post(TOKEN_URL)
            .form(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            if matches!(
                response.status(),
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::BAD_REQUEST
            ) {
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

        if token_response.access_token.trim().is_empty() {
            return Err(AuthError::InvalidResponse(
                "Empty access_token in refresh response".to_string(),
            ));
        }

        let expires_at =
            resolve_expires_at(token_response.expires_in, &token_response.access_token)?;

        let refresh_token = token_response
            .refresh_token
            .unwrap_or_else(|| refresh_token.to_string());

        Ok(AuthTokens {
            access_token: token_response.access_token,
            refresh_token,
            expires_at,
            id_token: token_response.id_token,
        })
    }
}

/// Check if tokens need refresh (within 5 minutes of expiry).
pub fn tokens_need_refresh(tokens: &AuthTokens) -> bool {
    match tokens.expires_at.duration_since(SystemTime::now()) {
        Ok(duration) => duration.as_secs() <= 300,
        Err(_) => true,
    }
}

/// Refresh tokens if needed, updating storage when refreshed.
pub async fn refresh_if_needed(
    storage: &Arc<dyn AuthStorage>,
    oauth_client: &OpenAIOAuth,
) -> Result<AuthTokens> {
    let credential = storage
        .get_credential(&provider::openai().storage_key(), CredentialType::OAuth2)
        .await?
        .ok_or(AuthError::ReauthRequired)?;

    let mut tokens = match credential {
        Credential::OAuth2(tokens) => tokens,
        _ => return Err(AuthError::ReauthRequired),
    };

    if tokens.id_token.is_none() || tokens_need_refresh(&tokens) {
        match oauth_client.refresh_tokens(&tokens.refresh_token).await {
            Ok(new_tokens) => {
                let merged_tokens = AuthTokens {
                    id_token: new_tokens.id_token.or(tokens.id_token),
                    ..new_tokens
                };
                storage
                    .set_credential("openai", Credential::OAuth2(merged_tokens.clone()))
                    .await?;
                tokens = merged_tokens;
            }
            Err(AuthError::ReauthRequired) => {
                storage
                    .remove_credential("openai", CredentialType::OAuth2)
                    .await?;
                return Err(AuthError::ReauthRequired);
            }
            Err(e) => return Err(e),
        }
    }

    if tokens.id_token.is_none() {
        return Err(AuthError::ReauthRequired);
    }

    Ok(tokens)
}

pub fn extract_chatgpt_account_id(id_token: &str) -> Result<ChatGptAccountId> {
    extract_chatgpt_account_id_from_id_token(id_token)
}

fn resolve_expires_at(expires_in: Option<u64>, access_token: &str) -> Result<SystemTime> {
    if let Some(expires_in) = expires_in {
        return Ok(SystemTime::now() + Duration::from_secs(expires_in));
    }

    let payload = decode_jwt_payload(access_token)?;
    let exp = payload
        .get("exp")
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|v| v as u64)))
        .ok_or_else(|| {
            AuthError::InvalidResponse("Missing exp claim in access token".to_string())
        })?;

    Ok(UNIX_EPOCH + Duration::from_secs(exp))
}

fn decode_jwt_payload(access_token: &str) -> Result<serde_json::Value> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() < 2 {
        return Err(AuthError::InvalidResponse(
            "Invalid access token format".to_string(),
        ));
    }

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| AuthError::InvalidResponse(format!("Invalid token payload: {e}")))?;

    serde_json::from_slice(&payload_bytes)
        .map_err(|e| AuthError::InvalidResponse(format!("Invalid token payload JSON: {e}")))
}
fn extract_chatgpt_account_id_from_id_token(id_token: &str) -> Result<ChatGptAccountId> {
    let payload = decode_jwt_payload(id_token)?;

    if let Some(account_id) = payload
        .get(CHATGPT_ACCOUNT_ID_NESTED_CLAIM)
        .and_then(|v| v.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Ok(ChatGptAccountId(account_id.to_string()));
    }

    if let Some(account_id) = payload
        .get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Ok(ChatGptAccountId(account_id.to_string()));
    }

    Err(AuthError::InvalidResponse(
        "Missing chatgpt account id in token".to_string(),
    ))
}

fn parse_callback_input(input: &str) -> Result<CallbackResponse> {
    let trimmed = input.trim();

    if trimmed.contains("code=") && trimmed.contains("state=") {
        let query = if trimmed.contains("://") {
            let url = url::Url::parse(trimmed)
                .map_err(|_| AuthError::InvalidCredential("Invalid redirect URL".to_string()))?;
            url.query().unwrap_or("").to_string()
        } else {
            trimmed.to_string()
        };

        let params: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();

        let code = params
            .get("code")
            .ok_or_else(|| AuthError::MissingInput("code parameter".to_string()))?;
        let state = params
            .get("state")
            .ok_or_else(|| AuthError::MissingInput("state parameter".to_string()))?;

        return Ok(CallbackResponse {
            code: code.to_string(),
            state: state.to_string(),
        });
    }

    if let Some((code, state)) = trimmed.split_once('#') {
        if code.is_empty() || state.is_empty() {
            return Err(AuthError::InvalidResponse(
                "Invalid callback code format".to_string(),
            ));
        }
        return Ok(CallbackResponse {
            code: code.to_string(),
            state: state.to_string(),
        });
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() == 2 {
        return Ok(CallbackResponse {
            code: parts[0].to_string(),
            state: parts[1].to_string(),
        });
    }

    Err(AuthError::InvalidResponse(
        "Invalid callback input. Paste the full redirect URL or code#state.".to_string(),
    ))
}

fn generate_random_string(length: usize) -> String {
    use rand::Rng;

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

#[derive(Debug)]
pub struct OpenAIAuthState {
    pub kind: OpenAIAuthStateKind,
}

#[derive(Debug)]
pub enum OpenAIAuthStateKind {
    OAuthStarted {
        verifier: String,
        state: String,
        auth_url: String,
        callback_server: Option<CallbackServerHandle>,
    },
}

pub struct OpenAIOAuthFlow {
    storage: Arc<dyn AuthStorage>,
    oauth_client: OpenAIOAuth,
}

impl OpenAIOAuthFlow {
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self {
            storage,
            oauth_client: OpenAIOAuth::new(),
        }
    }
}

#[async_trait]
impl AuthenticationFlow for OpenAIOAuthFlow {
    type State = OpenAIAuthState;

    fn available_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::OAuth]
    }

    async fn start_auth(&self, method: AuthMethod) -> Result<Self::State> {
        match method {
            AuthMethod::OAuth => {
                let pkce = OpenAIOAuth::generate_pkce();
                let state = OpenAIOAuth::generate_state();
                let auth_url = self.oauth_client.build_auth_url(&pkce, &state);

                let callback_server = match spawn_callback_server(
                    state.clone(),
                    SocketAddr::from(([127, 0, 0, 1], CALLBACK_PORT)),
                    CALLBACK_PATH,
                )
                .await
                {
                    Ok(handle) => Some(handle),
                    Err(err) => {
                        info!(
                            "OpenAI OAuth callback server unavailable, falling back to manual paste: {}",
                            err
                        );
                        None
                    }
                };

                Ok(OpenAIAuthState {
                    kind: OpenAIAuthStateKind::OAuthStarted {
                        verifier: pkce.verifier,
                        state,
                        auth_url,
                        callback_server,
                    },
                })
            }
            _ => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: provider::openai().storage_key(),
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
                let OpenAIAuthStateKind::OAuthStarted { auth_url, .. } = &state.kind;
                Ok(AuthProgress::OAuthStarted {
                    auth_url: auth_url.clone(),
                })
            }
            _ => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: provider::openai().storage_key(),
            }),
        }
    }

    async fn handle_input(&self, state: &mut Self::State, input: &str) -> Result<AuthProgress> {
        match &mut state.kind {
            OpenAIAuthStateKind::OAuthStarted {
                verifier,
                state: expected_state,
                callback_server,
                ..
            } => {
                let callback = if input.trim().is_empty() {
                    if let Some(server) = callback_server {
                        if let Some(result) = server.try_recv() {
                            result?
                        } else {
                            return Ok(AuthProgress::InProgress(
                                "Waiting for OAuth callback...".to_string(),
                            ));
                        }
                    } else {
                        return Ok(AuthProgress::NeedInput(
                            "Paste the redirect URL from your browser".to_string(),
                        ));
                    }
                } else {
                    parse_callback_input(input)?
                };

                if callback.state != *expected_state {
                    return Err(AuthError::StateMismatch);
                }

                let tokens = self
                    .oauth_client
                    .exchange_code_for_tokens(&callback.code, verifier)
                    .await?;

                self.storage
                    .set_credential("openai", Credential::OAuth2(tokens))
                    .await?;

                // Stop callback server if still running.
                if let Some(server) = callback_server.take() {
                    drop(server);
                }

                Ok(AuthProgress::Complete)
            }
        }
    }

    async fn is_authenticated(&self) -> Result<bool> {
        if let Some(Credential::OAuth2(tokens)) = self
            .storage
            .get_credential(&provider::openai().storage_key(), CredentialType::OAuth2)
            .await?
        {
            return Ok(tokens.id_token.is_some() && !tokens_need_refresh(&tokens));
        }

        Ok(false)
    }

    fn provider_name(&self) -> String {
        provider::openai().storage_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_url_building() {
        let oauth = OpenAIOAuth::new();
        let pkce = OpenAIOAuth::generate_pkce();
        let state = OpenAIOAuth::generate_state();

        let url = oauth.build_auth_url(&pkce, &state);

        assert!(url.contains(AUTHORIZE_URL));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains(&format!("originator={ORIGINATOR}")));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
    }

    #[test]
    fn test_parse_callback_input_url() {
        let input =
            "http://localhost:1455/auth/callback?code=abc123&state=state456";
        let parsed = parse_callback_input(input).unwrap();
        assert_eq!(parsed.code, "abc123");
        assert_eq!(parsed.state, "state456");
    }

    #[test]
    fn test_extract_chatgpt_account_id() {
        let payload = serde_json::json!({
            CHATGPT_ACCOUNT_ID_NESTED_CLAIM: {
                "chatgpt_account_id": "acct_123"
            },
            "exp": 1_700_000_000u64
        });
        let token = make_jwt(payload);
        let account_id = extract_chatgpt_account_id(&token).unwrap();
        assert_eq!(account_id.0, "acct_123");
    }

    #[test]
    fn test_extract_chatgpt_account_id_nested_claim() {
        let payload = serde_json::json!({
            CHATGPT_ACCOUNT_ID_NESTED_CLAIM: {
                "chatgpt_account_id": "acct_nested"
            },
            "exp": 1_700_000_000u64
        });
        let token = make_jwt(payload);
        let account_id = extract_chatgpt_account_id(&token).unwrap();
        assert_eq!(account_id.0, "acct_nested");
    }

    #[test]
    fn test_resolve_expires_at_from_token() {
        let payload = serde_json::json!({
            "chatgpt_account_id": "acct_123",
            "exp": 1_700_000_000u64
        });
        let token = make_jwt(payload);
        let exp = resolve_expires_at(None, &token).unwrap();
        assert_eq!(exp, UNIX_EPOCH + Duration::from_secs(1_700_000_000u64));
    }

    fn make_jwt(payload: serde_json::Value) -> String {
        let header = base64_url_encode(b"{}");
        let payload = base64_url_encode(payload.to_string().as_bytes());
        format!("{header}.{payload}.sig")
    }
}
