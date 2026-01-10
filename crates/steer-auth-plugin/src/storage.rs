use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

// Alias for backwards compatibility
pub type AuthTokens = OAuth2Token;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credential {
    #[serde(alias = "AuthTokens")]
    OAuth2(OAuth2Token),
    ApiKey {
        value: String,
    },
}

impl Credential {
    pub fn credential_type(&self) -> CredentialType {
        match self {
            Credential::OAuth2(_) => CredentialType::OAuth2,
            Credential::ApiKey { .. } => CredentialType::ApiKey,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CredentialType {
    #[serde(alias = "AuthTokens")]
    OAuth2,
    ApiKey,
}

impl std::fmt::Display for CredentialType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialType::OAuth2 => write!(f, "OAuth2"),
            CredentialType::ApiKey => write!(f, "ApiKey"),
        }
    }
}

#[async_trait]
pub trait AuthStorage: Send + Sync {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> crate::error::Result<Option<Credential>>;
    async fn set_credential(
        &self,
        provider: &str,
        credential: Credential,
    ) -> crate::error::Result<()>;
    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> crate::error::Result<()>;
}
