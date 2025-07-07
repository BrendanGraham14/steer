use comfy_table::{Cell, Color, Table};
use conductor_core::{
    api::ProviderKind,
    auth::{AuthStorage, CredentialType, DefaultAuthStorage},
    config::{LlmConfig, LlmConfigLoader},
};
use eyre::{Result, eyre};
use std::sync::Arc;

pub async fn execute() -> Result<()> {
    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("Provider").fg(Color::Green),
        Cell::new("Auth Method").fg(Color::Green),
        Cell::new("Status").fg(Color::Green),
    ]);

    // Load config to check for API keys
    let storage = Arc::new(
        DefaultAuthStorage::new().map_err(|e| eyre!("Failed to create auth storage: {}", e))?,
    );
    let loader = LlmConfigLoader::new(storage.clone());
    let config = loader
        .from_env()
        .await
        .map_err(|e| eyre!("Failed to load config: {}", e))?;

    // Check Anthropic/Claude
    add_anthropic_status(&mut table, &config).await?;

    // Check OpenAI
    add_other_provider_status(&mut table, "OpenAI", ProviderKind::OpenAI, &config);

    // Check Google/Gemini
    add_other_provider_status(&mut table, "Google (Gemini)", ProviderKind::Google, &config);

    println!("{table}");

    Ok(())
}

async fn add_anthropic_status(table: &mut Table, _config: &LlmConfig) -> Result<()> {
    // Check for API key
    let has_api_key =
        std::env::var("ANTHROPIC_API_KEY").is_ok() || std::env::var("CLAUDE_API_KEY").is_ok();

    // Check for OAuth tokens
    let storage = Arc::new(DefaultAuthStorage::new()?);
    let oauth_status = storage
        .get_credential("anthropic", CredentialType::AuthTokens)
        .await;

    let mut api_key_status = "❌ Not Configured";
    if has_api_key {
        api_key_status = "✅ Configured";
    }

    let mut oauth_status_str = "❌ Not Logged In".to_string();
    if let Ok(Some(credential)) = oauth_status {
        if let conductor_core::auth::Credential::AuthTokens(tokens) = credential {
            use std::time::SystemTime;
            let now = SystemTime::now();
            let expired = tokens.expires_at <= now;

            if expired {
                oauth_status_str = "⚠️ Tokens Expired".to_string();
            } else {
                let duration = tokens.expires_at.duration_since(now).unwrap_or_default();
                let minutes = duration.as_secs() / 60;
                oauth_status_str = format!("✅ Logged In ({minutes}m)");
            }
        }
    }

    table.add_row(vec![
        Cell::new("Anthropic (Claude)"),
        Cell::new("API Key"),
        Cell::new(api_key_status),
    ]);
    table.add_row(vec![
        Cell::new(""),
        Cell::new("OAuth"),
        Cell::new(&oauth_status_str),
    ]);

    // Precedence note
    if has_api_key && oauth_status_str != "❌ Not Logged In" {
        table.add_row(vec![
            Cell::new(""),
            Cell::new(""),
            Cell::new("(OAuth takes precedence)").fg(Color::DarkYellow),
        ]);
    }

    Ok(())
}

fn add_other_provider_status(
    table: &mut Table,
    display_name: &str,
    provider: ProviderKind,
    config: &LlmConfig,
) {
    let status = if config.auth_for(provider).is_some() {
        "✅ Configured"
    } else {
        "❌ Not Configured"
    };

    table.add_row(vec![
        Cell::new(display_name),
        Cell::new("API Key"),
        Cell::new(status),
    ]);
}
