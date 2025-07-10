use crate::api::ProviderKind;
use crate::auth::{AuthError, AuthStorage, AuthTokens, Credential, CredentialType, Result};
use crate::auth::{AuthMethod, AuthProgress, AuthenticationFlow};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// OAuth constants
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
        // Use the PKCE verifier as the state parameter
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

        let query = serde_urlencoded::to_string(params).unwrap();
        format!("{AUTHORIZE_URL}?{query}")
    }

    /// Parse the callback code from the redirect URL
    /// The format is: code#state
    pub fn parse_callback_code(callback_code: &str) -> Result<(String, String)> {
        let parts: Vec<&str> = callback_code.split('#').collect();
        if parts.len() != 2 {
            return Err(AuthError::InvalidResponse(
                "Invalid callback code format. Expected format: code#state".to_string(),
            ));
        }
        Ok((parts[0].to_string(), parts[1].to_string()))
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
        })
    }
}

/// Check if tokens need refresh (within 5 minutes of expiry)
pub fn tokens_need_refresh(tokens: &AuthTokens) -> bool {
    match tokens.expires_at.duration_since(SystemTime::now()) {
        Ok(duration) => duration.as_secs() <= 300, // 5 minutes
        Err(_) => true,                            // Already expired
    }
}

/// Get OAuth headers for Anthropic API requests
pub fn get_oauth_headers(access_token: &str) -> Vec<(String, String)> {
    vec![
        (
            "authorization".to_string(),
            format!("Bearer {access_token}"),
        ),
        ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
    ]
}

/// Helper to refresh tokens if needed
pub async fn refresh_if_needed(
    storage: &Arc<dyn AuthStorage>,
    oauth_client: &AnthropicOAuth,
) -> Result<AuthTokens> {
    let credential = storage
        .get_credential("anthropic", CredentialType::AuthTokens)
        .await?
        .ok_or(AuthError::ReauthRequired)?;

    let mut tokens = match credential {
        Credential::AuthTokens(tokens) => tokens,
        _ => return Err(AuthError::ReauthRequired),
    };

    if tokens_need_refresh(&tokens) {
        // Try to refresh
        match oauth_client.refresh_tokens(&tokens.refresh_token).await {
            Ok(new_tokens) => {
                storage
                    .set_credential("anthropic", Credential::AuthTokens(new_tokens.clone()))
                    .await?;
                tokens = new_tokens;
            }
            Err(AuthError::ReauthRequired) => {
                // Refresh token is invalid, clear tokens
                storage
                    .remove_credential("anthropic", CredentialType::AuthTokens)
                    .await?;
                return Err(AuthError::ReauthRequired);
            }
            Err(e) => return Err(e),
        }
    }

    Ok(tokens)
}

// Helper functions
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

/// State for the Anthropic authentication flow
#[derive(Debug, Clone)]
pub struct AnthropicAuthState {
    pub kind: AnthropicAuthStateKind,
}

#[derive(Debug, Clone)]
pub enum AnthropicAuthStateKind {
    /// Initial state - choosing auth method
    Initial,
    /// OAuth flow started, waiting for redirect URL
    OAuthStarted { verifier: String, auth_url: String },
    /// Waiting for API key input
    AwaitingApiKey,
}

/// Anthropic-specific authentication flow implementation
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
        vec![AuthMethod::OAuth, AuthMethod::ApiKey]
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
            AuthMethod::ApiKey => Ok(AnthropicAuthState {
                kind: AnthropicAuthStateKind::AwaitingApiKey,
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
                if let AnthropicAuthStateKind::OAuthStarted { auth_url, .. } = &state.kind {
                    Ok(AuthProgress::OAuthStarted {
                        auth_url: auth_url.clone(),
                    })
                } else {
                    Err(AuthError::InvalidState(
                        "Invalid state for OAuth".to_string(),
                    ))
                }
            }
            AuthMethod::ApiKey => Ok(AuthProgress::NeedInput("Enter your API key".to_string())),
        }
    }

    async fn handle_input(&self, state: &mut Self::State, input: &str) -> Result<AuthProgress> {
        match &mut state.kind {
            AnthropicAuthStateKind::Initial => Err(AuthError::InvalidState(
                "No input expected in initial state".to_string(),
            )),
            AnthropicAuthStateKind::OAuthStarted { verifier, .. } => {
                // Check if the input contains a redirect URL
                let (code, state_param) = if input.contains("code=") && input.contains("state=") {
                    // User pasted the full redirect URL
                    let url = reqwest::Url::parse(input).map_err(|_| {
                        AuthError::InvalidCredential("Invalid redirect URL".to_string())
                    })?;

                    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
                    let code = params
                        .get("code")
                        .ok_or_else(|| AuthError::MissingInput("code parameter".to_string()))?;
                    let state = params
                        .get("state")
                        .ok_or_else(|| AuthError::MissingInput("state parameter".to_string()))?;

                    (code.to_string(), state.to_string())
                } else {
                    // Legacy: try to parse as callback code
                    AnthropicOAuth::parse_callback_code(input)?
                };

                // Exchange code for tokens
                let tokens = self
                    .oauth_client
                    .exchange_code_for_tokens(&code, &state_param, verifier)
                    .await?;

                // Store the tokens
                self.storage
                    .set_credential("anthropic", Credential::AuthTokens(tokens))
                    .await?;

                Ok(AuthProgress::Complete)
            }
            AnthropicAuthStateKind::AwaitingApiKey => {
                if input.trim().is_empty() {
                    return Err(AuthError::InvalidCredential(
                        "API key cannot be empty".to_string(),
                    ));
                }

                // Store the API key
                self.storage
                    .set_credential(
                        "anthropic",
                        Credential::ApiKey {
                            value: input.to_string(),
                        },
                    )
                    .await?;

                Ok(AuthProgress::Complete)
            }
        }
    }

    async fn is_authenticated(&self) -> Result<bool> {
        // Check for OAuth tokens first
        if let Some(Credential::AuthTokens(tokens)) = self
            .storage
            .get_credential("anthropic", CredentialType::AuthTokens)
            .await?
        {
            // Check if tokens are still valid
            return Ok(!tokens_need_refresh(&tokens));
        }

        // Check for API key
        Ok(self
            .storage
            .get_credential("anthropic", CredentialType::ApiKey)
            .await?
            .is_some())
    }

    fn provider_name(&self) -> String {
        ProviderKind::Anthropic.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthStorage, Credential, CredentialType};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    #[test]
    fn test_pkce_generation() {
        let pkce = AnthropicOAuth::generate_pkce();

        // Verifier should be 128 characters
        assert_eq!(pkce.verifier.len(), 128);

        // Challenge should be base64url encoded SHA256 (43 chars)
        assert_eq!(pkce.challenge.len(), 43);

        // Verify challenge is correctly derived from verifier
        let expected_challenge = base64_url_encode(&sha256(&pkce.verifier));
        assert_eq!(pkce.challenge, expected_challenge);
    }

    #[test]
    fn test_state_generation() {
        let pkce = AnthropicOAuth::generate_pkce();
        // State is now the PKCE verifier
        assert_eq!(pkce.verifier.len(), 128);
    }

    #[test]
    fn test_auth_url_building() {
        let oauth = AnthropicOAuth::new();
        let pkce = AnthropicOAuth::generate_pkce();

        let url = oauth.build_auth_url(&pkce);

        assert!(url.contains(AUTHORIZE_URL));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains("response_type=code"));
        // The verifier might contain URL-unsafe characters that get encoded
        assert!(url.contains("state="));
        assert!(url.contains(&format!("code_challenge={}", &pkce.challenge)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code=true"));
        // Verify redirect URI is properly encoded
        assert!(url.contains(
            "redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"
        ));
    }

    #[test]
    fn test_parse_callback_code() {
        // Valid format
        let (code, state) = AnthropicOAuth::parse_callback_code("abc123#xyz789").unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");

        // Invalid format - no hash
        assert!(AnthropicOAuth::parse_callback_code("abc123").is_err());

        // Invalid format - multiple hashes
        assert!(AnthropicOAuth::parse_callback_code("abc#123#xyz").is_err());
    }

    /// Mock implementation of AuthStorage for testing
    struct MockAuthStorage {
        credentials: Arc<Mutex<HashMap<String, Credential>>>,
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
            _provider: &str,
            _credential_type: CredentialType,
        ) -> Result<Option<Credential>> {
            Ok(None)
        }

        async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
            let mut creds = self.credentials.lock().await;
            creds.insert(provider.to_string(), credential);
            Ok(())
        }

        async fn remove_credential(
            &self,
            provider: &str,
            _credential_type: CredentialType,
        ) -> Result<()> {
            let mut creds = self.credentials.lock().await;
            creds.remove(provider);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_auth_flow_api_key() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = AnthropicOAuthFlow::new(storage.clone());

        // Test available methods
        let methods = auth_flow.available_methods();
        assert_eq!(methods.len(), 2);
        assert!(methods.contains(&AuthMethod::OAuth));
        assert!(methods.contains(&AuthMethod::ApiKey));

        // Start API key flow
        let state = auth_flow.start_auth(AuthMethod::ApiKey).await.unwrap();
        assert!(matches!(state.kind, AnthropicAuthStateKind::AwaitingApiKey));

        // Handle API key input
        let mut state = state;
        let progress = auth_flow
            .handle_input(&mut state, "test-api-key")
            .await
            .unwrap();
        assert!(matches!(progress, AuthProgress::Complete));

        // Verify API key was stored
        let creds = storage.credentials.lock().await;
        assert!(creds.contains_key("anthropic"));
        if let Some(Credential::ApiKey { value }) = creds.get("anthropic") {
            assert_eq!(value, "test-api-key");
        } else {
            panic!("Expected API key credential");
        }
    }

    #[tokio::test]
    async fn test_auth_flow_oauth_start() {
        let storage = Arc::new(MockAuthStorage::new());
        let auth_flow = AnthropicOAuthFlow::new(storage);

        // Start OAuth flow
        let state = auth_flow.start_auth(AuthMethod::OAuth).await.unwrap();

        if let AnthropicAuthStateKind::OAuthStarted { auth_url, verifier } = &state.kind {
            // Verify auth URL contains required parameters
            assert!(auth_url.contains(AUTHORIZE_URL));
            assert!(auth_url.contains("client_id="));
            assert!(auth_url.contains("code_challenge="));
            assert!(!verifier.is_empty());
        } else {
            panic!("Expected OAuth started state");
        }
    }
}
