use crate::api::ProviderKind;
use crate::auth::{AuthStorage, CredentialType, Result};
use std::collections::HashSet;

/// Check which providers have valid authentication configured
pub async fn get_authenticated_providers(
    auth_storage: &dyn AuthStorage,
) -> Result<HashSet<ProviderKind>> {
    let mut providers = HashSet::new();

    // Check Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("CLAUDE_API_KEY").is_ok()
        || auth_storage
            .get_credential("anthropic", CredentialType::AuthTokens)
            .await?
            .is_some()
        || auth_storage
            .get_credential("anthropic", CredentialType::ApiKey)
            .await?
            .is_some()
    {
        providers.insert(ProviderKind::Anthropic);
    }

    // Check OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok()
        || auth_storage
            .get_credential("openai", CredentialType::ApiKey)
            .await?
            .is_some()
    {
        providers.insert(ProviderKind::OpenAI);
    }

    // Check Gemini/Google
    if std::env::var("GEMINI_API_KEY").is_ok()
        || auth_storage
            .get_credential("google", CredentialType::ApiKey)
            .await?
            .is_some()
    {
        providers.insert(ProviderKind::Google);
    }

    // Check Grok
    if std::env::var("GROK_API_KEY").is_ok()
        || auth_storage
            .get_credential("grok", CredentialType::ApiKey)
            .await?
            .is_some()
    {
        providers.insert(ProviderKind::Grok);
    }

    Ok(providers)
}
