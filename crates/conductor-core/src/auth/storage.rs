use crate::auth::error::{AuthError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: SystemTime,
}

#[async_trait]
pub trait AuthStorage: Send + Sync {
    async fn get_tokens(&self, provider: &str) -> Result<Option<AuthTokens>>;
    async fn set_tokens(&self, provider: &str, tokens: AuthTokens) -> Result<()>;
    async fn remove_tokens(&self, provider: &str) -> Result<()>;
}

/// Primary storage using OS keyring
pub struct KeyringStorage {
    service_prefix: String,
}

impl Default for KeyringStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyringStorage {
    pub fn new() -> Self {
        Self {
            service_prefix: "conductor".to_string(),
        }
    }

    fn service_name(&self, provider: &str) -> String {
        format!("{}-{}", self.service_prefix, provider)
    }

    fn get_username() -> String {
        whoami::username()
    }
}

#[async_trait]
impl AuthStorage for KeyringStorage {
    async fn get_tokens(&self, provider: &str) -> Result<Option<AuthTokens>> {
        let service = self.service_name(provider);
        let username = Self::get_username();

        // Run blocking keyring operation in a spawn_blocking task
        let result = tokio::task::spawn_blocking(
            move || -> std::result::Result<Option<AuthTokens>, keyring::Error> {
                let entry = keyring::Entry::new(&service, &username)?;
                let password = match entry.get_password() {
                    Ok(pwd) => pwd,
                    Err(keyring::Error::NoEntry) => return Ok(None),
                    Err(e) => return Err(e),
                };

                let tokens: AuthTokens = match serde_json::from_str(&password) {
                    Ok(tokens) => tokens,
                    Err(_) => return Err(keyring::Error::NoEntry), // Use NoEntry as a generic error
                };
                Ok(Some(tokens))
            },
        )
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?;

        result.map_err(AuthError::from)
    }

    async fn set_tokens(&self, provider: &str, tokens: AuthTokens) -> Result<()> {
        let service = self.service_name(provider);
        let username = Self::get_username();
        let password = serde_json::to_string(&tokens)
            .map_err(|e| AuthError::Storage(format!("Failed to serialize tokens: {e}")))?;

        // Run blocking keyring operation in a spawn_blocking task
        tokio::task::spawn_blocking(move || -> std::result::Result<(), keyring::Error> {
            let entry = keyring::Entry::new(&service, &username)?;
            entry.set_password(&password)?;
            Ok(())
        })
        .await
        .map_err(|e| AuthError::Storage(format!("Task join error: {e}")))?
        .map_err(AuthError::from)
    }

    async fn remove_tokens(&self, provider: &str) -> Result<()> {
        let service = self.service_name(provider);
        let username = Self::get_username();

        // Run blocking keyring operation in a spawn_blocking task
        tokio::task::spawn_blocking(move || -> std::result::Result<(), keyring::Error> {
            let entry = keyring::Entry::new(&service, &username)?;
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
            file_path: conductor_dir.join("tokens.json.enc"),
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
}

#[async_trait]
impl AuthStorage for EncryptedFileStorage {
    async fn get_tokens(&self, provider: &str) -> Result<Option<AuthTokens>> {
        let encrypted_data = self.read_encrypted_data().await?;
        if encrypted_data.is_empty() {
            return Ok(None);
        }

        let decrypted = self.decrypt(&encrypted_data)?;
        let all_tokens: std::collections::HashMap<String, AuthTokens> =
            serde_json::from_slice(&decrypted)
                .map_err(|e| AuthError::Storage(format!("Failed to parse tokens: {e}")))?;

        Ok(all_tokens.get(provider).cloned())
    }

    async fn set_tokens(&self, provider: &str, tokens: AuthTokens) -> Result<()> {
        // Read existing tokens
        let mut all_tokens: std::collections::HashMap<String, AuthTokens> =
            match self.get_tokens("").await {
                Ok(None) => std::collections::HashMap::new(),
                Ok(Some(_)) => {
                    // Need to read all tokens
                    let encrypted_data = self.read_encrypted_data().await?;
                    if encrypted_data.is_empty() {
                        std::collections::HashMap::new()
                    } else {
                        let decrypted = self.decrypt(&encrypted_data)?;
                        serde_json::from_slice(&decrypted).map_err(|e| {
                            AuthError::Storage(format!("Failed to parse tokens: {e}"))
                        })?
                    }
                }
                Err(_) => std::collections::HashMap::new(),
            };

        // Update tokens for this provider
        all_tokens.insert(provider.to_string(), tokens);

        // Serialize and encrypt
        let serialized = serde_json::to_vec(&all_tokens)
            .map_err(|e| AuthError::Storage(format!("Failed to serialize tokens: {e}")))?;
        let encrypted = self.encrypt(&serialized)?;

        // Write to file
        self.write_encrypted_data(&encrypted).await?;
        Ok(())
    }

    async fn remove_tokens(&self, provider: &str) -> Result<()> {
        // Read existing tokens
        let encrypted_data = self.read_encrypted_data().await?;
        if encrypted_data.is_empty() {
            return Ok(()); // Nothing to remove
        }

        let decrypted = self.decrypt(&encrypted_data)?;
        let mut all_tokens: std::collections::HashMap<String, AuthTokens> =
            serde_json::from_slice(&decrypted)
                .map_err(|e| AuthError::Storage(format!("Failed to parse tokens: {e}")))?;

        // Remove tokens for this provider
        all_tokens.remove(provider);

        if all_tokens.is_empty() {
            // Delete the file if no tokens remain
            if let Err(e) = tokio::fs::remove_file(&self.file_path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(e.into());
                }
            }
        } else {
            // Re-encrypt and write remaining tokens
            let serialized = serde_json::to_vec(&all_tokens)
                .map_err(|e| AuthError::Storage(format!("Failed to serialize tokens: {e}")))?;
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
            Some(Arc::new(KeyringStorage::new()) as Arc<dyn AuthStorage>)
        } else {
            None
        };

        // Always create file storage as fallback
        let file = Arc::new(EncryptedFileStorage::new()?) as Arc<dyn AuthStorage>;

        Ok(Self { keyring, file })
    }
}

#[async_trait]
impl AuthStorage for DefaultAuthStorage {
    async fn get_tokens(&self, provider: &str) -> Result<Option<AuthTokens>> {
        // Try keyring first
        if let Some(keyring) = &self.keyring {
            match keyring.get_tokens(provider).await {
                Ok(tokens) => return Ok(tokens),
                Err(AuthError::Keyring(_)) => {
                    // Keyring failed, fall back to file
                }
                Err(e) => return Err(e),
            }
        }

        // Fall back to file storage
        self.file.get_tokens(provider).await
    }

    async fn set_tokens(&self, provider: &str, tokens: AuthTokens) -> Result<()> {
        // Try keyring first
        if let Some(keyring) = &self.keyring {
            match keyring.set_tokens(provider, tokens.clone()).await {
                Ok(()) => return Ok(()),
                Err(AuthError::Keyring(_)) => {
                    // Keyring failed, fall back to file
                }
                Err(e) => return Err(e),
            }
        }

        // Fall back to file storage
        self.file.set_tokens(provider, tokens).await
    }

    async fn remove_tokens(&self, provider: &str) -> Result<()> {
        let mut any_error = None;

        // Try to remove from both storages
        if let Some(keyring) = &self.keyring {
            if let Err(e) = keyring.remove_tokens(provider).await {
                any_error = Some(e);
            }
        }

        if let Err(e) = self.file.remove_tokens(provider).await {
            any_error = Some(e);
        }

        if let Some(e) = any_error {
            Err(e)
        } else {
            Ok(())
        }
    }
}
