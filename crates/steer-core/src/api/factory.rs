use crate::api::error::ApiError;
use crate::api::provider::Provider;
use crate::api::{
    claude::AnthropicClient, gemini::GeminiClient, openai::{CodexClient, OpenAIClient}, xai::XAIClient,
};
use crate::auth::storage::Credential;
use crate::config::provider::{ApiFormat, ProviderConfig};
use std::sync::Arc;

/// Factory function to create a provider instance based on the provider config and credential.
///
/// This function dispatches to the correct API client implementation based on the provider's
/// API format. It also supports base URL overrides for custom providers using compatible
/// API formats.
pub fn create_provider(
    provider_cfg: &ProviderConfig,
    credential: &Credential,
) -> Result<Arc<dyn Provider>, ApiError> {
    match credential {
        Credential::ApiKey { value } => match &provider_cfg.api_format {
            ApiFormat::OpenaiResponses => {
                let client = if let Some(base_url) = &provider_cfg.base_url {
                    OpenAIClient::with_base_url_mode(
                        value.clone(),
                        Some(base_url.to_string()),
                        crate::api::openai::OpenAIMode::Responses,
                    )
                } else {
                    OpenAIClient::with_mode(
                        value.clone(),
                        crate::api::openai::OpenAIMode::Responses,
                    )
                };
                Ok(Arc::new(client))
            }
            ApiFormat::OpenaiChat => {
                let client = if let Some(base_url) = &provider_cfg.base_url {
                    OpenAIClient::with_base_url_mode(
                        value.clone(),
                        Some(base_url.to_string()),
                        crate::api::openai::OpenAIMode::Chat,
                    )
                } else {
                    OpenAIClient::with_mode(value.clone(), crate::api::openai::OpenAIMode::Chat)
                };
                Ok(Arc::new(client))
            }
            ApiFormat::Anthropic => {
                // TODO: Add base_url support to AnthropicClient
                if provider_cfg.base_url.is_some() {
                    return Err(ApiError::Configuration(
                        "Base URL override not yet supported for Anthropic API format".to_string(),
                    ));
                }
                Ok(Arc::new(AnthropicClient::with_api_key(value)))
            }
            ApiFormat::Google => {
                // TODO: Add base_url support to GeminiClient
                if provider_cfg.base_url.is_some() {
                    return Err(ApiError::Configuration(
                        "Base URL override not yet supported for Gemini API format".to_string(),
                    ));
                }
                Ok(Arc::new(GeminiClient::new(value)))
            }
            ApiFormat::Xai => {
                let client = if let Some(base_url) = &provider_cfg.base_url {
                    XAIClient::with_base_url(value.clone(), Some(base_url.to_string()))
                } else {
                    XAIClient::new(value.clone())
                };
                Ok(Arc::new(client))
            }
        },
        Credential::OAuth2(_) => {
            // Only Anthropic supports OAuth currently
            match &provider_cfg.api_format {
                ApiFormat::Anthropic => {
                    // OAuth for Anthropic requires the storage, which we don't have here
                    // This will be handled differently in the refactored code
                    Err(ApiError::Configuration(
                        "OAuth support requires auth storage context".to_string(),
                    ))
                }
                _ => Err(ApiError::Configuration(format!(
                    "OAuth is not supported for {:?} API format",
                    provider_cfg.api_format
                ))),
            }
        }
    }
}

/// Factory function that creates a provider with OAuth support (requires storage).
///
/// This is a separate function because OAuth providers need access to the auth storage
/// to refresh tokens.
pub fn create_provider_with_storage(
    provider_cfg: &ProviderConfig,
    credential: &Credential,
    storage: Arc<dyn crate::auth::AuthStorage>,
) -> Result<Arc<dyn Provider>, ApiError> {
    match credential {
        Credential::ApiKey { .. } => create_provider(provider_cfg, credential),
        Credential::OAuth2(_) => {
            if provider_cfg.id == crate::config::provider::openai() {
                if provider_cfg.base_url.is_some() {
                    return Err(ApiError::Configuration(
                        "Base URL override not supported with OpenAI OAuth".to_string(),
                    ));
                }
                if provider_cfg.api_format != ApiFormat::OpenaiResponses {
                    return Err(ApiError::Configuration(
                        "OpenAI OAuth is only supported with responses API format".to_string(),
                    ));
                }
                return Ok(Arc::new(CodexClient::new(storage)));
            }

            match &provider_cfg.api_format {
                ApiFormat::Anthropic => {
                    if provider_cfg.base_url.is_some() {
                        return Err(ApiError::Configuration(
                            "Base URL override not supported with OAuth authentication".to_string(),
                        ));
                    }
                    Ok(Arc::new(AnthropicClient::with_oauth(storage)))
                }
                _ => Err(ApiError::Configuration(format!(
                    "OAuth is not supported for {:?} API format",
                    provider_cfg.api_format
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider::{self, AuthScheme, ProviderId};

    #[test]
    fn test_create_openai_provider() {
        let config = ProviderConfig {
            id: provider::openai(),
            name: "OpenAI".to_string(),
            api_format: ApiFormat::OpenaiResponses,
            auth_schemes: vec![AuthScheme::ApiKey],
            base_url: None,
        };

        let credential = Credential::ApiKey {
            value: "test-key".to_string(),
        };

        let provider = create_provider(&config, &credential).unwrap();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn test_create_custom_openai_provider() {
        let config = ProviderConfig {
            id: ProviderId("my-provider".to_string()),
            name: "My Provider".to_string(),
            api_format: ApiFormat::OpenaiResponses,
            auth_schemes: vec![AuthScheme::ApiKey],
            base_url: Some("https://my-api.example.com".parse().unwrap()),
        };

        let credential = Credential::ApiKey {
            value: "test-key".to_string(),
        };

        let provider = create_provider(&config, &credential).unwrap();
        assert_eq!(provider.name(), "openai"); // Still uses OpenAI client
    }

    #[test]
    fn test_oauth_requires_storage() {
        let config = ProviderConfig {
            id: provider::anthropic(),
            name: "Anthropic".to_string(),
            api_format: ApiFormat::Anthropic,
            auth_schemes: vec![AuthScheme::Oauth2],
            base_url: None,
        };

        let credential = Credential::OAuth2(crate::auth::storage::OAuth2Token {
            access_token: "test-token".to_string(),
            refresh_token: "test-refresh".to_string(),
            expires_at: std::time::SystemTime::now(),
        });

        let result = create_provider(&config, &credential);
        assert!(result.is_err());
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(err_msg.contains("OAuth support requires auth storage"));
    }
}
