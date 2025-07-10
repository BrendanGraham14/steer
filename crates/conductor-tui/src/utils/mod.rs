use crate::error::Result;
use conductor_core::api::{Model, ProviderKind};
use conductor_core::auth::{AuthStorage, inspect::get_authenticated_providers};
use conductor_core::preferences::Preferences;

/// Select a default model based on available authentication and user preferences
pub async fn select_default_model(
    auth_storage: &dyn AuthStorage,
    preferred_model: Option<Model>,
) -> Result<Model> {
    let authenticated_providers = get_authenticated_providers(auth_storage)
        .await
        .map_err(|e| crate::error::Error::Core(conductor_core::error::Error::Auth(e)))?;

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
                "anthropic" => ProviderKind::Anthropic,
                "openai" => ProviderKind::OpenAI,
                "google" | "gemini" => ProviderKind::Google,
                "grok" => ProviderKind::Grok,
                _ => continue, // Skip unknown providers
            };

            if authenticated_providers.contains(&provider) {
                // Return first available model from this provider
                match provider {
                    ProviderKind::Anthropic => return Ok(Model::default()),
                    ProviderKind::OpenAI => return Ok(Model::Gpt4_1_20250414),
                    ProviderKind::Google => return Ok(Model::Gemini2_5ProPreview0605),
                    ProviderKind::Grok => return Ok(Model::Grok3),
                }
            }
        }
    }

    // Otherwise, use default priority order: Anthropic, OpenAI, Google, Grok
    if authenticated_providers.contains(&ProviderKind::Anthropic) {
        return Ok(Model::default()); // Default is Claude Opus 4
    }

    if authenticated_providers.contains(&ProviderKind::OpenAI) {
        return Ok(Model::Gpt4_1_20250414);
    }

    if authenticated_providers.contains(&ProviderKind::Google) {
        return Ok(Model::Gemini2_5ProPreview0605); // The one with "gemini" alias
    }

    if authenticated_providers.contains(&ProviderKind::Grok) {
        return Ok(Model::Grok3);
    }

    // Default to Claude if no providers are authenticated
    Ok(Model::default())
}
