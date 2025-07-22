use crate::error::Result;
use steer_core::api::{Model, ProviderKind};
use steer_core::auth::{AuthStorage, inspect::get_authenticated_providers};
use steer_core::preferences::Preferences;

/// Select a default model based on available authentication and user preferences
pub async fn select_default_model(
    auth_storage: &dyn AuthStorage,
    preferred_model: Option<Model>,
) -> Result<Model> {
    let authenticated_providers = get_authenticated_providers(auth_storage)
        .await
        .map_err(|e| crate::error::Error::Core(steer_core::error::Error::Auth(e)))?;

    // If preferred model is specified and its provider is authenticated, use it
    if let Some(model) = preferred_model {
        if authenticated_providers.contains(&model.provider()) {
            return Ok(model);
        }
    }

    // Load preferences to check provider priority
    let preferences = Preferences::load()
        .map_err(crate::error::Error::Core)
        .unwrap_or_default();

    // If provider priority is specified, use it
    if let Some(priority) = preferences.ui.provider_priority {
        for provider_str in priority {
            let provider = match provider_str.as_str() {
                "anthropic" | "claude" => ProviderKind::Anthropic,
                "openai" => ProviderKind::OpenAI,
                "google" | "gemini" => ProviderKind::Google,
                "xai" | "grok" => ProviderKind::XAI,
                _ => continue, // Skip unknown providers
            };

            if authenticated_providers.contains(&provider) {
                // Return first available model from this provider
                match provider {
                    ProviderKind::Anthropic => return Ok(Model::ClaudeOpus4_20250514),
                    ProviderKind::OpenAI => return Ok(Model::O3_20250416),
                    ProviderKind::Google => return Ok(Model::Gemini2_5ProPreview0605),
                    ProviderKind::XAI => return Ok(Model::Grok4_0709),
                }
            }
        }
    }

    // Otherwise, use default priority order: Anthropic, OpenAI, Google, xAI
    if authenticated_providers.contains(&ProviderKind::Anthropic) {
        return Ok(Model::ClaudeOpus4_20250514); // Default is Claude Opus 4
    }

    if authenticated_providers.contains(&ProviderKind::OpenAI) {
        return Ok(Model::O3_20250416);
    }

    if authenticated_providers.contains(&ProviderKind::Google) {
        return Ok(Model::Gemini2_5ProPreview0605);
    }

    if authenticated_providers.contains(&ProviderKind::XAI) {
        return Ok(Model::Grok4_0709);
    }

    // Default to Claude if no providers are authenticated
    Ok(Model::default())
}
