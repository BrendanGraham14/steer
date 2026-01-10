use crate::auth::error::{AuthError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
pub use steer_auth_plugin::storage::{
    AuthStorage, AuthTokens, Credential, CredentialType, OAuth2Token,
};

/// Collection of all credentials kept in the keyring. The first key is the
/// provider id (e.g. `"anthropic"`), the second key is the credential type
/// (`"AuthTokens"` / `"ApiKey"`). Each leaf holds the raw `Credential` value
/// for that pair.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CredentialStore(HashMap<String, HashMap<CredentialType, Credential>>);

/// Primary storage using OS keyring
pub struct KeyringStorage {
    service_name: String,
}

impl Default for KeyringStorage {
    fn default() -> Self {
        Self::new("steer")
    }
}

impl KeyringStorage {
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
        }
    }

    fn get_username() -> String {
        whoami::username()
    }
}

#[async_trait]
impl AuthStorage for KeyringStorage {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<Option<Credential>> {
        let provider = provider.to_string();
        let username = Self::get_username();
        let service = self.service_name.clone();
        let cred_type = credential_type;

        // Load, parse and query the credential store
        let result = tokio::task::spawn_blocking(
            move || -> std::result::Result<Option<Credential>, keyring::Error> {
                let entry = keyring::Entry::new(&service, &username)?;
                let store_json = match entry.get_password() {
                    Ok(pwd) => pwd,
                    Err(keyring::Error::NoEntry) => return Ok(None),
                    Err(e) => return Err(e),
                };

                let store: CredentialStore = serde_json::from_str(&store_json).unwrap_or_default();

                // Get the credential with the requested type
                // The serde aliases handle migration from old "AuthTokens" to "OAuth2" automatically
                let cred = store
                    .0
                    .get(&provider)
                    .and_then(|m| m.get(&cred_type))
                    .cloned();

                Ok(cred)
            },
        )
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?;

        result.map_err(AuthError::from)
    }

    async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
        let service = self.service_name.clone();
        let username = Self::get_username();
        let provider = provider.to_string();
        let cred_type = credential.credential_type();

        tokio::task::spawn_blocking(move || -> std::result::Result<(), keyring::Error> {
            let entry = keyring::Entry::new(&service, &username)?;
            // Load existing store (if any)
            let mut store: CredentialStore = match entry.get_password() {
                Ok(pwd) => serde_json::from_str(&pwd).unwrap_or_default(),
                Err(keyring::Error::NoEntry) => CredentialStore::default(),
                Err(e) => return Err(e),
            };

            // Update
            store
                .0
                .entry(provider)
                .or_default()
                .insert(cred_type, credential);

            let data = serde_json::to_string(&store).expect("serialize credential store");
            entry.set_password(&data)?;
            Ok(())
        })
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?
        .map_err(AuthError::from)
    }

    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<()> {
        let service = self.service_name.clone();
        let username = Self::get_username();
        let provider = provider.to_string();

        tokio::task::spawn_blocking(move || -> std::result::Result<(), keyring::Error> {
            let entry = keyring::Entry::new(&service, &username)?;

            // Load existing store, return Ok if none
            let store_json = match entry.get_password() {
                Ok(pwd) => pwd,
                Err(keyring::Error::NoEntry) => return Ok(()),
                Err(e) => return Err(e),
            };

            let mut store: CredentialStore = serde_json::from_str(&store_json).unwrap_or_default();

            if let Some(map) = store.0.get_mut(&provider) {
                map.remove(&credential_type);
                if map.is_empty() {
                    store.0.remove(&provider);
                }
            }

            if store.0.is_empty() {
                // No credentials left – remove the keyring entry entirely.
                let _ = entry.delete_credential();
            } else {
                let data = serde_json::to_string(&store).expect("serialize credential store");
                entry.set_password(&data)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?
        .map_err(AuthError::from)
    }
}

/// Default storage implementation that tries keyring first, then falls back to encrypted file
pub struct DefaultAuthStorage {
    keyring: Arc<dyn AuthStorage>,
}

impl DefaultAuthStorage {
    pub fn new() -> Result<Self> {
        // Try to create keyring storage
        if !cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )) {
            return Err(AuthError::Storage(
                "Keyring not supported on this platform".to_string(),
            ));
        }

        let keyring = Arc::new(KeyringStorage::new("steer")) as Arc<dyn AuthStorage>;

        Ok(Self { keyring })
    }

    // Convenience methods for working with specific credential types
    pub async fn get_auth_tokens(&self, provider: &str) -> Result<Option<OAuth2Token>> {
        match self
            .get_credential(provider, CredentialType::OAuth2)
            .await?
        {
            Some(Credential::OAuth2(tokens)) => Ok(Some(tokens)),
            _ => Ok(None),
        }
    }

    pub async fn set_auth_tokens(&self, provider: &str, tokens: OAuth2Token) -> Result<()> {
        self.set_credential(provider, Credential::OAuth2(tokens))
            .await
    }

    pub async fn get_api_key(&self, provider: &str) -> Result<Option<String>> {
        match self
            .get_credential(provider, CredentialType::ApiKey)
            .await?
        {
            Some(Credential::ApiKey { value }) => Ok(Some(value)),
            _ => Ok(None),
        }
    }

    pub async fn set_api_key(&self, provider: &str, api_key: String) -> Result<()> {
        self.set_credential(provider, Credential::ApiKey { value: api_key })
            .await
    }

    pub async fn remove_auth_tokens(&self, provider: &str) -> Result<()> {
        self.remove_credential(provider, CredentialType::OAuth2)
            .await
    }

    pub async fn remove_api_key(&self, provider: &str) -> Result<()> {
        self.remove_credential(provider, CredentialType::ApiKey)
            .await
    }
}

#[async_trait]
impl AuthStorage for DefaultAuthStorage {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<Option<Credential>> {
        self.keyring.get_credential(provider, credential_type).await
    }

    async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
        self.keyring
            .set_credential(provider, credential.clone())
            .await
    }

    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<()> {
        self.keyring
            .remove_credential(provider, credential_type)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn test_credential_deserialization_with_alias() {
        // Test that old "AuthTokens" format deserializes correctly to OAuth2
        let old_json_format = r#"{
            "anthropic": {
                "AuthTokens": {
                    "type": "AuthTokens",
                    "access_token": "old_access_token",
                    "refresh_token": "old_refresh_token",
                    "expires_at": {
                        "secs_since_epoch": 1678886400,
                        "nanos_since_epoch": 0
                    }
                }
            }
        }"#;

        let store: CredentialStore =
            serde_json::from_str(old_json_format).expect("Failed to deserialize old format");

        // The serde alias should have converted AuthTokens to OAuth2
        let creds = store.0.get("anthropic").unwrap();
        let cred = creds.get(&CredentialType::OAuth2).unwrap();

        match cred {
            Credential::OAuth2(token) => {
                assert_eq!(token.access_token, "old_access_token");
                assert_eq!(token.refresh_token, "old_refresh_token");
                assert_eq!(
                    token.expires_at,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(1678886400)
                );
            }
            _ => panic!("Deserialization failed: expected OAuth2 credential"),
        }
    }

    #[test]
    fn test_credential_type_deserialization_with_alias() {
        // Test that the old "AuthTokens" string deserializes to OAuth2
        let old_type = r#""AuthTokens""#;
        let cred_type: CredentialType = serde_json::from_str(old_type).unwrap();
        assert_eq!(cred_type, CredentialType::OAuth2);

        // Also test that "OAuth2" works
        let new_type = r#""OAuth2""#;
        let cred_type: CredentialType = serde_json::from_str(new_type).unwrap();
        assert_eq!(cred_type, CredentialType::OAuth2);
    }
}
