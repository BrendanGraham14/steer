use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for session store creation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionStoreConfig {
    /// SQLite database store
    Sqlite {
        /// Path to the database file
        path: PathBuf,
    },
    /// Future: PostgreSQL store
    #[allow(dead_code)]
    Postgres {
        /// Connection string
        connection_string: String,
    },
    /// Future: In-memory store for testing
    #[allow(dead_code)]
    Memory,
}

impl SessionStoreConfig {
    /// Create a new SQLite store configuration
    pub fn sqlite(path: PathBuf) -> Self {
        Self::Sqlite { path }
    }

    /// Get the default session store configuration
    pub fn default_sqlite() -> Result<Self> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            Error::Configuration("Could not determine home directory".to_string())
        })?;
        let db_path = home_dir.join(".steer").join("sessions.db");
        Ok(Self::sqlite(db_path))
    }
}

impl Default for SessionStoreConfig {
    fn default() -> Self {
        Self::default_sqlite().unwrap_or_else(|_| {
            // Fallback to current directory if home directory cannot be determined
            Self::sqlite(PathBuf::from("./sessions.db"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite_config_creation() {
        let path = PathBuf::from("/tmp/test.db");
        let config = SessionStoreConfig::sqlite(path.clone());

        let SessionStoreConfig::Sqlite { path: config_path } = config else {
            unreachable!("expected SQLite config")
        };
        assert_eq!(config_path, path);
    }

    #[test]
    fn test_default_sqlite_config() {
        let config = SessionStoreConfig::default_sqlite();
        assert!(config.is_ok());

        let SessionStoreConfig::Sqlite { path } = config.unwrap() else {
            unreachable!("expected SQLite config")
        };
        assert!(path.to_string_lossy().contains(".steer"));
        assert!(path.to_string_lossy().contains("sessions.db"));
    }

    #[test]
    fn test_default_config() {
        let config = SessionStoreConfig::default();

        let SessionStoreConfig::Sqlite { path } = config else {
            unreachable!("expected SQLite config")
        };
        // Should either be in .steer or current directory
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("sessions.db"));
    }
}
