use crate::auth::error::{AuthError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
use strum::Display;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credential {
    AuthTokens(AuthTokens),
    ApiKey { value: String },
}

impl Credential {
    pub fn credential_type(&self) -> CredentialType {
        match self {
            Credential::AuthTokens(_) => CredentialType::AuthTokens,
            Credential::ApiKey { .. } => CredentialType::ApiKey,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Display, PartialEq, Eq, Hash)]
pub enum CredentialType {
    AuthTokens,
    ApiKey,
}

#[async_trait]
pub trait AuthStorage: Send + Sync {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<Option<Credential>>;
    async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()>;
    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<()>;
}

/// Primary storage using OS keyring
pub struct KeyringStorage {
    service_name: String,
}

impl Default for KeyringStorage {
    fn default() -> Self {
        Self::new("conductor")
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

    fn get_target(provider: &str, credential_type: CredentialType) -> String {
        format!("{provider}-{credential_type}")
    }
}

#[async_trait]
impl AuthStorage for KeyringStorage {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<Option<Credential>> {
        let service = self.service_name.clone();
        let username = Self::get_username();
        let target = Self::get_target(provider, credential_type);

        // Run blocking keyring operation in a spawn_blocking task
        let result = tokio::task::spawn_blocking(
            move || -> std::result::Result<Option<Credential>, keyring::Error> {
                let entry = keyring::Entry::new_with_target(&target, &service, &username)?;
                let password = match entry.get_password() {
                    Ok(pwd) => pwd,
                    Err(keyring::Error::NoEntry) => return Ok(None),
                    Err(e) => return Err(e),
                };

                let credential: Credential = match serde_json::from_str(&password) {
                    Ok(credential) => credential,
                    Err(_) => return Err(keyring::Error::NoEntry), // Use NoEntry as a generic error
                };
                Ok(Some(credential))
            },
        )
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?;

        result.map_err(AuthError::from)
    }

    async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
        let service = self.service_name.clone();
        let username = Self::get_username();
        let target = Self::get_target(provider, credential.credential_type());
        let password = serde_json::to_string(&credential)
            .map_err(|e| AuthError::Storage(format!("Failed to serialize credential: {e}")))?;

        // Run blocking keyring operation in a spawn_blocking task
        tokio::task::spawn_blocking(move || -> std::result::Result<(), keyring::Error> {
            let entry = keyring::Entry::new_with_target(&target, &service, &username)?;
            entry.set_password(&password)?;
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
        let target = Self::get_target(provider, credential_type);

        // Run blocking keyring operation in a spawn_blocking task
        tokio::task::spawn_blocking(move || -> std::result::Result<(), keyring::Error> {
            let entry = keyring::Entry::new_with_target(&target, &service, &username)?;
            match entry.delete_credential() {
                Ok(()) => Ok(()),
                Err(keyring::Error::NoEntry) => Ok(()), // Already removed
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?
        .map_err(AuthError::from)
    }
}

/// Fallback storage using encrypted file
pub struct EncryptedFileStorage {
    file_path: std::path::PathBuf,
}

impl EncryptedFileStorage {
    pub fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| AuthError::Storage("Unable to find config directory".to_string()))?;
        let conductor_dir = config_dir.join("conductor");
        std::fs::create_dir_all(&conductor_dir)?;

        Ok(Self {
            file_path: conductor_dir.join("credentials.json.enc"),
        })
    }

    fn get_encryption_key() -> Result<[u8; 32]> {
        use hkdf::Hkdf;
        use sha2::Sha256;

        let username = whoami::username();
        let hostname = gethostname::gethostname().to_string_lossy().to_string();
        let salt = format!("{username}-{hostname}");

        let mut key = [0u8; 32];
        let hkdf = Hkdf::<Sha256>::new(Some(salt.as_bytes()), b"conductor-oauth");
        hkdf.expand(b"encryption-key", &mut key)
            .map_err(|e| AuthError::Encryption(format!("Key derivation failed: {e}")))?;

        Ok(key)
    }

    async fn read_encrypted_data(&self) -> Result<Vec<u8>> {
        match tokio::fs::read(&self.file_path).await {
            Ok(data) => Ok(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    async fn write_encrypted_data(&self, data: &[u8]) -> Result<()> {
        tokio::fs::write(&self.file_path, data).await?;
        Ok(())
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            Aes256Gcm, Nonce,
            aead::{Aead, KeyInit, OsRng},
        };
        use rand::Rng;

        let key = Self::get_encryption_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| AuthError::Encryption(format!("Failed to create cipher: {e}")))?;

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| AuthError::Encryption(format!("Encryption failed: {e}")))?;

        // Prepend nonce to ciphertext
        let mut result = nonce_bytes.to_vec();
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            Aes256Gcm, Nonce,
            aead::{Aead, KeyInit},
        };

        if data.len() < 12 {
            return Err(AuthError::Encryption("Invalid encrypted data".to_string()));
        }

        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let key = Self::get_encryption_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| AuthError::Encryption(format!("Failed to create cipher: {e}")))?;

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| AuthError::Encryption(format!("Decryption failed: {e}")))
    }

    fn make_key(provider: &str, credential_type: CredentialType) -> String {
        format!("{provider}-{credential_type}")
    }
}

#[async_trait]
impl AuthStorage for EncryptedFileStorage {
    async fn get_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<Option<Credential>> {
        let encrypted_data = self.read_encrypted_data().await?;
        if encrypted_data.is_empty() {
            return Ok(None);
        }

        let decrypted = self.decrypt(&encrypted_data)?;
        let all_credentials: std::collections::HashMap<String, Credential> =
            serde_json::from_slice(&decrypted)
                .map_err(|e| AuthError::Storage(format!("Failed to parse credentials: {e}")))?;

        let key = Self::make_key(provider, credential_type);
        Ok(all_credentials.get(&key).cloned())
    }

    async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
        // Read existing credentials
        let mut all_credentials: std::collections::HashMap<String, Credential> = if let Ok(data) =
            self.read_encrypted_data().await
        {
            if data.is_empty() {
                std::collections::HashMap::new()
            } else {
                let decrypted = self.decrypt(&data)?;
                serde_json::from_slice(&decrypted)
                    .map_err(|e| AuthError::Storage(format!("Failed to parse credentials: {e}")))?
            }
        } else {
            std::collections::HashMap::new()
        };

        // Update credentials for this provider and type
        let key = Self::make_key(provider, credential.credential_type());
        all_credentials.insert(key, credential);

        // Serialize and encrypt
        let serialized = serde_json::to_vec(&all_credentials)
            .map_err(|e| AuthError::Storage(format!("Failed to serialize credentials: {e}")))?;
        let encrypted = self.encrypt(&serialized)?;

        // Write to file
        self.write_encrypted_data(&encrypted).await?;
        Ok(())
    }

    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<()> {
        // Read existing credentials
        let encrypted_data = self.read_encrypted_data().await?;
        if encrypted_data.is_empty() {
            return Ok(()); // Nothing to remove
        }

        let decrypted = self.decrypt(&encrypted_data)?;
        let mut all_credentials: std::collections::HashMap<String, Credential> =
            serde_json::from_slice(&decrypted)
                .map_err(|e| AuthError::Storage(format!("Failed to parse credentials: {e}")))?;

        // Remove credentials for this provider and type
        let key = Self::make_key(provider, credential_type);
        all_credentials.remove(&key);

        if all_credentials.is_empty() {
            // Delete the file if no credentials remain
            if let Err(e) = tokio::fs::remove_file(&self.file_path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(e.into());
                }
            }
        } else {
            // Re-encrypt and write remaining credentials
            let serialized = serde_json::to_vec(&all_credentials)
                .map_err(|e| AuthError::Storage(format!("Failed to serialize credentials: {e}")))?;
            let encrypted = self.encrypt(&serialized)?;
            self.write_encrypted_data(&encrypted).await?;
        }

        Ok(())
    }
}

/// Default storage implementation that tries keyring first, then falls back to encrypted file
pub struct DefaultAuthStorage {
    keyring: Option<Arc<dyn AuthStorage>>,
    file: Arc<dyn AuthStorage>,
}

impl DefaultAuthStorage {
    pub fn new() -> Result<Self> {
        // Try to create keyring storage
        let keyring = if cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )) {
            Some(Arc::new(KeyringStorage::new("conductor")) as Arc<dyn AuthStorage>)
        } else {
            None
        };

        // Always create file storage as fallback
        let file = Arc::new(EncryptedFileStorage::new()?) as Arc<dyn AuthStorage>;

        Ok(Self { keyring, file })
    }

    // Convenience methods for working with specific credential types
    pub async fn get_auth_tokens(&self, provider: &str) -> Result<Option<AuthTokens>> {
        match self
            .get_credential(provider, CredentialType::AuthTokens)
            .await?
        {
            Some(Credential::AuthTokens(tokens)) => Ok(Some(tokens)),
            _ => Ok(None),
        }
    }

    pub async fn set_auth_tokens(&self, provider: &str, tokens: AuthTokens) -> Result<()> {
        self.set_credential(provider, Credential::AuthTokens(tokens))
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
        self.remove_credential(provider, CredentialType::AuthTokens)
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
        // Try keyring first
        if let Some(keyring) = &self.keyring {
            match keyring.get_credential(provider, credential_type).await {
                Ok(credential) => return Ok(credential),
                Err(AuthError::Keyring(_)) => {
                    // Keyring failed, fall back to file
                }
                Err(e) => return Err(e),
            }
        }

        // Fall back to file storage
        self.file.get_credential(provider, credential_type).await
    }

    async fn set_credential(&self, provider: &str, credential: Credential) -> Result<()> {
        // Try keyring first
        if let Some(keyring) = &self.keyring {
            match keyring.set_credential(provider, credential.clone()).await {
                Ok(()) => return Ok(()),
                Err(AuthError::Keyring(_)) => {
                    // Keyring failed, fall back to file
                }
                Err(e) => return Err(e),
            }
        }

        // Fall back to file storage
        self.file.set_credential(provider, credential).await
    }

    async fn remove_credential(
        &self,
        provider: &str,
        credential_type: CredentialType,
    ) -> Result<()> {
        let mut any_error = None;

        // Try to remove from both storages
        if let Some(keyring) = &self.keyring {
            if let Err(e) = keyring.remove_credential(provider, credential_type).await {
                any_error = Some(e);
            }
        }

        if let Err(e) = self.file.remove_credential(provider, credential_type).await {
            any_error = Some(e);
        }

        if let Some(e) = any_error {
            Err(e)
        } else {
            Ok(())
        }
    }
}
