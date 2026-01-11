use crate::api::error::ApiError;
use crate::api::provider::Provider;
use crate::api::{
    claude::AnthropicClient, gemini::GeminiClient, openai::OpenAIClient, xai::XAIClient,
};
use crate::auth::{AuthDirective, Credential};
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
        Credential::OAuth2(_) => Err(ApiError::Configuration(
            "OAuth requires an AuthDirective, not a raw credential".to_string(),
        )),
    }
}

/// Factory function that creates a provider using an auth directive.
pub fn create_provider_with_directive(
    provider_cfg: &ProviderConfig,
    directive: &AuthDirective,
) -> Result<Arc<dyn Provider>, ApiError> {
    match directive {
        AuthDirective::OpenAiResponses(openai) => {
            if provider_cfg.api_format != ApiFormat::OpenaiResponses {
                return Err(ApiError::Configuration(
                    "OpenAI OAuth directives require responses API format".to_string(),
                ));
            }
            let base_url = provider_cfg.base_url.as_ref().map(|url| url.to_string());
            Ok(Arc::new(OpenAIClient::with_directive(
                openai.clone(),
                base_url,
            )))
        }
        AuthDirective::Anthropic(anthropic) => {
            if provider_cfg.api_format != ApiFormat::Anthropic {
                return Err(ApiError::Configuration(
                    "Anthropic OAuth directives require Anthropic API format".to_string(),
                ));
            }
            if provider_cfg.base_url.is_some() {
                return Err(ApiError::Configuration(
                    "Base URL override not yet supported for Anthropic API format".to_string(),
                ));
            }
            Ok(Arc::new(AnthropicClient::with_directive(anthropic.clone())))
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
    fn test_oauth_requires_directive() {
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
            id_token: None,
        });

        let result = create_provider(&config, &credential);
        assert!(result.is_err());
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(err_msg.contains("OAuth requires an AuthDirective"));
    }
}
