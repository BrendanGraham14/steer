use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

use steer_auth_plugin::AuthPlugin;
use steer_auth_plugin::{
    AuthDirective, AuthError, AuthErrorAction, AuthErrorContext, AuthHeaderContext,
    AuthHeaderProvider, AuthMethod, AuthProgress, AuthSource, AuthStorage, AuthTokens,
    AuthenticationFlow, Credential, CredentialType, DynAuthenticationFlow, HeaderPair,
    InstructionPolicy, ModelId, ModelVisibilityPolicy, OpenAiResponsesAuth, ProviderId, Result,
};
use steer_tools::tools::{
    AST_GREP_TOOL_NAME, BASH_TOOL_NAME, DISPATCH_AGENT_TOOL_NAME, EDIT_TOOL_NAME, FETCH_TOOL_NAME,
    GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, MULTI_EDIT_TOOL_NAME, READ_FILE_TOOL_NAME,
    REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME,
};

mod callback_server;
use callback_server::{CallbackResponse, CallbackServerHandle, spawn_callback_server};

const PROVIDER_ID: &str = "openai";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPES: &str = "openid profile email offline_access";
const ORIGINATOR: &str = "codex_cli_rs";
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_PORT: u16 = 1455;

const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_BETA: &str = "responses=experimental";
const GPT_5_2_CODEX_MODEL_ID: &str = "gpt-5.2-codex";
const GPT_5_3_CODEX_MODEL_ID: &str = "gpt-5.3-codex";
const CODEX_SYSTEM_PROMPT: &str = r#"You are Codex, based on GPT-5. You are running as a coding agent in the Codex CLI on a user's computer.

## General

- When searching for text or files, prefer using `rg` or `rg --files` respectively because `rg` is much faster than alternatives like `grep`. (If the `rg` command is not found, then use alternatives.)

## Editing constraints

- Default to ASCII when editing or creating files. Only introduce non-ASCII or other Unicode characters when there is a clear justification and the file already uses them.
- Add succinct code comments that explain what is going on if code is not self-explanatory. You should not add comments like "Assigns the value to the variable", but a brief comment might be useful ahead of a complex code block that the user would otherwise have to spend time parsing out. Usage of these comments should be rare.
- Try to use apply_patch for single file edits, but it is fine to explore other options to make the edit if it does not work well. Do not use apply_patch for changes that are auto-generated (i.e. generating package.json or running a lint or format command like gofmt) or when scripting is more efficient (such as search and replacing a string across a codebase).
- You may be in a dirty git worktree.
    * NEVER revert existing changes you did not make unless explicitly requested, since these changes were made by the user.
    * If asked to make a commit or code edits and there are unrelated changes to your work or changes that you didn't make in those files, don't revert those changes.
    * If the changes are in files you've touched recently, you should read carefully and understand how you can work with the changes rather than reverting them.
    * If the changes are in unrelated files, just ignore them and don't revert them.
- Do not amend a commit unless explicitly requested to do so.
- While you are working, you might notice unexpected changes that you didn't make. If this happens, STOP IMMEDIATELY and ask the user how they would like to proceed.
- **NEVER** use destructive commands like `git reset --hard` or `git checkout --` unless specifically requested or approved by the user.

## Plan tool

When using the planning tool:
- Skip using the planning tool for straightforward tasks (roughly the easiest 25%).
- Do not make single-step plans.
- When you made a plan, update it after having performed one of the sub-tasks that you shared on the plan.

## Special user requests

- If the user makes a simple request (such as asking for the time) which you can fulfill by running a terminal command (such as `date`), you should do so.
- If the user asks for a "review", default to a code review mindset: prioritise identifying bugs, risks, behavioural regressions, and missing tests. Findings must be the primary focus of the response - keep summaries or overviews brief and only after enumerating the issues. Present findings first (ordered by severity with file/line references), follow with open questions or assumptions, and offer a change-summary only as a secondary detail. If no findings are discovered, state that explicitly and mention any residual risks or testing gaps.

## Frontend tasks
When doing frontend design tasks, avoid collapsing into "AI slop" or safe, average-looking layouts.
Aim for interfaces that feel intentional, bold, and a bit surprising.
- Typography: Use expressive, purposeful fonts and avoid default stacks (Inter, Roboto, Arial, system).
- Color & Look: Choose a clear visual direction; define CSS variables; avoid purple-on-white defaults. No purple bias or dark mode bias.
- Motion: Use a few meaningful animations (page-load, staggered reveals) instead of generic micro-motions.
- Background: Don't rely on flat, single-color backgrounds; use gradients, shapes, or subtle patterns to build atmosphere.
- Overall: Avoid boilerplate layouts and interchangeable UI patterns. Vary themes, type families, and visual languages across outputs.
- Ensure the page loads properly on both desktop and mobile

Exception: If working within an existing website or design system, preserve the established patterns, structure, and visual language.

## Presenting your work and final message

You are producing plain text that will later be styled by the CLI. Follow these rules exactly. Formatting should make results easy to scan, but not feel mechanical. Use judgment to decide how much structure adds value.

- Default: be very concise; friendly coding teammate tone.
- Ask only when needed; suggest ideas; mirror the user's style.
- For substantial work, summarize clearly; follow final‑answer formatting.
- Skip heavy formatting for simple confirmations.
- Don't dump large files you've written; reference paths only.
- No "save/copy this file" - User is on the same machine.
- Offer logical next steps (tests, commits, build) briefly; add verify steps if you couldn't do something.
- For code changes:
  * Lead with a quick explanation of the change, and then give more details on the context covering where and why a change was made. Do not start this explanation with "summary", just jump right in.
  * If there are natural next steps the user may want to take, suggest them at the end of your response. Do not make suggestions if there are no natural next steps.
  * When suggesting multiple options, use numeric lists for the suggestions so the user can quickly respond with a single number.
- The user does not command execution outputs. When asked to show the output of a command (e.g. `git show`), relay the important details in your answer or summarize the key lines so the user understands the result.

### Final answer structure and style guidelines

- Plain text; CLI handles styling. Use structure only when it helps scanability.
- Headers: optional; short Title Case (1-3 words) wrapped in **…**; no blank line before the first bullet; add only if they truly help.
- Bullets: use - ; merge related points; keep to one line when possible; 4–6 per list ordered by importance; keep phrasing consistent.
- Monospace: backticks for commands/paths/env vars/code ids and inline examples; use for literal keyword bullets; never combine with **.
- Code samples or multi-line snippets should be wrapped in fenced code blocks; include an info string as often as possible.
- Structure: group related bullets; order sections general → specific → supporting; for subsections, start with a bolded keyword bullet, then items; match complexity to the task.
- Tone: collaborative, concise, factual; present tense, active voice; self‑contained; no "above/below"; parallel wording.
- Don'ts: no nested bullets/hierarchies; no ANSI codes; don't cram unrelated keywords; keep keyword lists short—wrap/reformat if long; avoid naming formatting styles in answers.
- Adaptation: code explanations → precise, structured with code refs; simple tasks → lead with outcome; big changes → logical walkthrough + rationale + next actions; casual one-offs → plain sentences, no headers/bullets.
- File References: When referencing files in your response follow the below rules:
  * Use inline code to make file paths clickable.
  * Each reference should have a stand alone path. Even if it's the same file.
  * Accepted: absolute, workspace‑relative, a/ or b/ diff prefixes, or bare filename/suffix.
  * Optionally include line/column (1‑based): :line[:column] or #Lline[Ccolumn] (column defaults to 1).
  * Do not use URIs like file://, vscode://, or https://.
  * Do not provide range of lines
  * Examples: src/app.ts, src/app.ts:42, b/server/index.js#L10, C:\repo\project\main.rs:12:5
"#;

fn steer_codex_bridge_prompt() -> String {
    format!(
        r"## Codex Running in Steer

You are running Codex inside Steer, an open-source terminal coding assistant.

### CRITICAL tool replacements
- apply_patch does NOT exist. Use `{EDIT_TOOL_NAME}` instead.
- update_plan does NOT exist. Use `{TODO_WRITE_TOOL_NAME}` instead.
- read_plan does NOT exist. Use `{TODO_READ_TOOL_NAME}` instead.

### Steer tool names
- File: `{READ_FILE_TOOL_NAME}`, `{REPLACE_TOOL_NAME}`, `{EDIT_TOOL_NAME}`, `{MULTI_EDIT_TOOL_NAME}`
- Search: `{GREP_TOOL_NAME}` (text), `{AST_GREP_TOOL_NAME}` (syntax), `{GLOB_TOOL_NAME}` (paths), `{LS_TOOL_NAME}` (list directories)
- Exec: `{BASH_TOOL_NAME}`
- Web: `{FETCH_TOOL_NAME}`
- Agents: `{DISPATCH_AGENT_TOOL_NAME}`
- Todos: `{TODO_READ_TOOL_NAME}`, `{TODO_WRITE_TOOL_NAME}`

Tool names are case-sensitive; use exact casing.

### File path rules
- `{READ_FILE_TOOL_NAME}`, `{REPLACE_TOOL_NAME}`, `{EDIT_TOOL_NAME}`, `{MULTI_EDIT_TOOL_NAME}`, and `{LS_TOOL_NAME}` require absolute paths.

### Edit semantics
- `{EDIT_TOOL_NAME}` uses exact string replacement (empty `old_string` creates a file).
- `{MULTI_EDIT_TOOL_NAME}` applies multiple exact replacements in a single file.
- `{REPLACE_TOOL_NAME}` overwrites the entire file contents.

### Search guidance
- Prefer `{GREP_TOOL_NAME}`/`{AST_GREP_TOOL_NAME}`/`{GLOB_TOOL_NAME}`/`{LS_TOOL_NAME}` over shelling out to `rg` via `{BASH_TOOL_NAME}`.

### Todo guidance
- Use `{TODO_READ_TOOL_NAME}`/`{TODO_WRITE_TOOL_NAME}` for complex or multi-step tasks; skip them for simple, single-step work unless the user asks.
",
    )
}

fn codex_instructions() -> String {
    format!("{CODEX_SYSTEM_PROMPT}\n\n{}", steer_codex_bridge_prompt())
}

const CHATGPT_ACCOUNT_ID_NESTED_CLAIM: &str = "https://api.openai.com/auth";

#[derive(Debug)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatGptAccountId(pub String);

#[derive(Clone)]
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
        .get_credential(PROVIDER_ID, CredentialType::OAuth2)
        .await?
        .ok_or(AuthError::ReauthRequired)?;

    let mut tokens = match credential {
        Credential::OAuth2(tokens) => tokens,
        Credential::ApiKey { .. } => return Err(AuthError::ReauthRequired),
    };

    if tokens.id_token.is_none() || tokens_need_refresh(&tokens) {
        match oauth_client.refresh_tokens(&tokens.refresh_token).await {
            Ok(new_tokens) => {
                let merged_tokens = AuthTokens {
                    id_token: new_tokens.id_token.or(tokens.id_token),
                    ..new_tokens
                };
                storage
                    .set_credential(PROVIDER_ID, Credential::OAuth2(merged_tokens.clone()))
                    .await?;
                tokens = merged_tokens;
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

    if tokens.id_token.is_none() {
        return Err(AuthError::ReauthRequired);
    }

    Ok(tokens)
}

async fn force_refresh(
    storage: &Arc<dyn AuthStorage>,
    oauth_client: &OpenAIOAuth,
) -> Result<AuthTokens> {
    let credential = storage
        .get_credential(PROVIDER_ID, CredentialType::OAuth2)
        .await?
        .ok_or(AuthError::ReauthRequired)?;

    let tokens = match credential {
        Credential::OAuth2(tokens) => tokens,
        Credential::ApiKey { .. } => return Err(AuthError::ReauthRequired),
    };

    match oauth_client.refresh_tokens(&tokens.refresh_token).await {
        Ok(new_tokens) => {
            let merged_tokens = AuthTokens {
                id_token: new_tokens.id_token.or(tokens.id_token),
                ..new_tokens
            };
            storage
                .set_credential(PROVIDER_ID, Credential::OAuth2(merged_tokens.clone()))
                .await?;
            Ok(merged_tokens)
        }
        Err(AuthError::ReauthRequired) => {
            storage
                .remove_credential(PROVIDER_ID, CredentialType::OAuth2)
                .await?;
            Err(AuthError::ReauthRequired)
        }
        Err(e) => Err(e),
    }
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
            code: code.clone(),
            state: state.clone(),
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
            AuthMethod::ApiKey => Err(AuthError::UnsupportedMethod {
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
                let OpenAIAuthStateKind::OAuthStarted { auth_url, .. } = &state.kind;
                Ok(AuthProgress::OAuthStarted {
                    auth_url: auth_url.clone(),
                })
            }
            AuthMethod::ApiKey => Err(AuthError::UnsupportedMethod {
                method: format!("{method:?}"),
                provider: PROVIDER_ID.to_string(),
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
                    .set_credential(PROVIDER_ID, Credential::OAuth2(tokens))
                    .await?;

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
            .get_credential(PROVIDER_ID, CredentialType::OAuth2)
            .await?
        {
            return Ok(tokens.id_token.is_some() && !tokens_need_refresh(&tokens));
        }

        Ok(false)
    }

    fn provider_name(&self) -> String {
        PROVIDER_ID.to_string()
    }
}

#[derive(Clone)]
struct OpenAiHeaderProvider {
    storage: Arc<dyn AuthStorage>,
    oauth: OpenAIOAuth,
}

impl OpenAiHeaderProvider {
    fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self {
            storage,
            oauth: OpenAIOAuth::new(),
        }
    }

    async fn header_pairs(&self, _ctx: AuthHeaderContext) -> Result<Vec<HeaderPair>> {
        let tokens = refresh_if_needed(&self.storage, &self.oauth).await?;
        let id_token = tokens
            .id_token
            .as_deref()
            .ok_or(AuthError::ReauthRequired)?;
        let account_id = extract_chatgpt_account_id(id_token)?;

        Ok(vec![
            HeaderPair {
                name: "authorization".to_string(),
                value: format!("Bearer {}", tokens.access_token),
            },
            HeaderPair {
                name: "chatgpt-account-id".to_string(),
                value: account_id.0,
            },
            HeaderPair {
                name: "openai-beta".to_string(),
                value: OPENAI_BETA.to_string(),
            },
            HeaderPair {
                name: "originator".to_string(),
                value: ORIGINATOR.to_string(),
            },
        ])
    }
}

#[async_trait]
impl AuthHeaderProvider for OpenAiHeaderProvider {
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

struct OpenAiModelVisibility;

impl ModelVisibilityPolicy for OpenAiModelVisibility {
    fn allow_model(&self, model_id: &ModelId, auth_source: &AuthSource) -> bool {
        if model_id.provider_id.0 != PROVIDER_ID {
            return true;
        }

        if matches!(
            model_id.model_id.as_str(),
            GPT_5_2_CODEX_MODEL_ID | GPT_5_3_CODEX_MODEL_ID
        ) {
            return matches!(auth_source, AuthSource::Plugin { .. });
        }

        true
    }
}

#[derive(Clone)]
pub struct OpenAiAuthPlugin;

impl Default for OpenAiAuthPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiAuthPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AuthPlugin for OpenAiAuthPlugin {
    fn provider_id(&self) -> ProviderId {
        ProviderId(PROVIDER_ID.to_string())
    }

    fn supported_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::OAuth]
    }

    fn create_flow(&self, storage: Arc<dyn AuthStorage>) -> Option<Box<dyn DynAuthenticationFlow>> {
        Some(Box::new(steer_auth_plugin::AuthFlowWrapper::new(
            OpenAIOAuthFlow::new(storage),
        )))
    }

    async fn resolve_auth(&self, storage: Arc<dyn AuthStorage>) -> Result<Option<AuthDirective>> {
        let is_authenticated = self.is_authenticated(storage.clone()).await?;
        if !is_authenticated {
            return Ok(None);
        }

        let headers = Arc::new(OpenAiHeaderProvider::new(storage));
        let directive = OpenAiResponsesAuth {
            headers,
            base_url_override: Some(CODEX_BASE_URL.to_string()),
            require_streaming: Some(true),
            instruction_policy: Some(InstructionPolicy::Override(codex_instructions())),
            include: Some(vec!["reasoning.encrypted_content".to_string()]),
        };

        Ok(Some(AuthDirective::OpenAiResponses(directive)))
    }

    async fn is_authenticated(&self, storage: Arc<dyn AuthStorage>) -> Result<bool> {
        if let Some(Credential::OAuth2(tokens)) = storage
            .get_credential(PROVIDER_ID, CredentialType::OAuth2)
            .await?
        {
            return Ok(tokens.id_token.is_some() && !tokens_need_refresh(&tokens));
        }

        Ok(false)
    }

    fn model_visibility(&self) -> Option<Box<dyn ModelVisibilityPolicy>> {
        Some(Box::new(OpenAiModelVisibility))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_auth_plugin::{AuthMethod, AuthSource};

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
        let input = "http://localhost:1455/auth/callback?code=abc123&state=state456";
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

    #[test]
    fn test_openai_codex_models_require_plugin_auth() {
        let visibility = OpenAiModelVisibility;
        let codex_5_2 = ModelId {
            provider_id: ProviderId(PROVIDER_ID.to_string()),
            model_id: GPT_5_2_CODEX_MODEL_ID.to_string(),
        };
        let codex_5_3 = ModelId {
            provider_id: ProviderId(PROVIDER_ID.to_string()),
            model_id: GPT_5_3_CODEX_MODEL_ID.to_string(),
        };

        assert!(visibility.allow_model(
            &codex_5_2,
            &AuthSource::Plugin {
                method: AuthMethod::OAuth,
            }
        ));
        assert!(visibility.allow_model(
            &codex_5_3,
            &AuthSource::Plugin {
                method: AuthMethod::OAuth,
            }
        ));
        assert!(!visibility.allow_model(
            &codex_5_2,
            &AuthSource::ApiKey {
                origin: steer_auth_plugin::ApiKeyOrigin::Env,
            }
        ));
        assert!(!visibility.allow_model(
            &codex_5_3,
            &AuthSource::ApiKey {
                origin: steer_auth_plugin::ApiKeyOrigin::Stored,
            }
        ));
    }

    fn make_jwt(payload: serde_json::Value) -> String {
        let header = base64_url_encode(b"{}");
        let payload = base64_url_encode(payload.to_string().as_bytes());
        format!("{header}.{payload}.sig")
    }
}
