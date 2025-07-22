use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::session::{
    Session, SessionStoreConfig,
    state::{SessionConfig, SessionToolConfig, ToolApprovalPolicy, WorkspaceConfig},
    store::SessionStore,
};

pub fn create_session_store_path() -> Result<std::path::PathBuf> {
    let home_dir = dirs::home_dir()
        .ok_or_else(|| Error::Configuration("Could not determine home directory".to_string()))?;
    let db_path = home_dir.join(".steer").join("sessions.db");
    Ok(db_path)
}

/// Resolve session store configuration from an optional path
/// If no path is provided, uses the default SQLite configuration
pub fn resolve_session_store_config(
    session_db_path: Option<PathBuf>,
) -> Result<SessionStoreConfig> {
    match session_db_path {
        Some(path) => Ok(SessionStoreConfig::sqlite(path)),
        None => SessionStoreConfig::default_sqlite()
            .map_err(|e| Error::Configuration(format!("Failed to get default sqlite config: {e}"))),
    }
}

pub async fn create_session_store() -> Result<Arc<dyn SessionStore>> {
    let config = SessionStoreConfig::default();
    create_session_store_with_config(config).await
}

pub async fn create_session_store_with_config(
    config: SessionStoreConfig,
) -> Result<Arc<dyn SessionStore>> {
    use crate::session::stores::sqlite::SqliteSessionStore;

    match config {
        SessionStoreConfig::Sqlite { path } => {
            // Create directory if it doesn't exist
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let store = SqliteSessionStore::new(&path).await?;

            Ok(Arc::new(store))
        }
        _ => Err(Error::Configuration(
            "Unsupported session store type".to_string(),
        )),
    }
}

pub fn create_default_session_config() -> SessionConfig {
    SessionConfig {
        workspace: WorkspaceConfig::default(),
        tool_config: SessionToolConfig::default(),
        system_prompt: None,
        metadata: HashMap::new(),
    }
}

pub fn parse_tool_policy(
    policy_str: &str,
    pre_approved_tools: Option<&str>,
) -> Result<ToolApprovalPolicy> {
    match policy_str {
        "always_ask" => Ok(ToolApprovalPolicy::AlwaysAsk),
        "pre_approved" => {
            let tools = if let Some(tools_str) = pre_approved_tools {
                tools_str.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                return Err(Error::Configuration(
                    "pre_approved_tools is required when using pre_approved policy".to_string(),
                ));
            };
            Ok(ToolApprovalPolicy::PreApproved { tools })
        }
        "mixed" => {
            let tools = if let Some(tools_str) = pre_approved_tools {
                tools_str.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                std::collections::HashSet::new()
            };
            Ok(ToolApprovalPolicy::Mixed {
                pre_approved: tools,
                ask_for_others: true,
            })
        }
        _ => Err(Error::Configuration(format!(
            "Invalid tool policy: {policy_str}. Valid options: always_ask, pre_approved, mixed"
        ))),
    }
}

pub fn parse_metadata(metadata_str: Option<&str>) -> Result<HashMap<String, String>> {
    let mut metadata = HashMap::new();

    if let Some(meta_str) = metadata_str {
        for pair in meta_str.split(',') {
            let parts: Vec<&str> = pair.split('=').collect();
            if parts.len() != 2 {
                return Err(Error::Configuration(
                    "Invalid metadata format. Expected key=value pairs separated by commas"
                        .to_string(),
                ));
            }
            metadata.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
    }

    Ok(metadata)
}

pub fn create_mock_session(id: &str, tool_policy: ToolApprovalPolicy) -> Session {
    let tool_config = SessionToolConfig {
        approval_policy: tool_policy,
        ..Default::default()
    };

    let config = SessionConfig {
        workspace: WorkspaceConfig::default(),
        tool_config,
        system_prompt: None,
        metadata: std::collections::HashMap::new(),
    };
    Session::new(id.to_string(), config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_session_store_config_with_path() {
        let custom_path = PathBuf::from("/custom/path/sessions.db");
        let config = resolve_session_store_config(Some(custom_path.clone())).unwrap();

        match config {
            SessionStoreConfig::Sqlite { path } => {
                assert_eq!(path, custom_path);
            }
            _ => unreachable!("SQLite config"),
        }
    }

    #[test]
    fn test_resolve_session_store_config_without_path() {
        let config = resolve_session_store_config(None).unwrap();

        match config {
            SessionStoreConfig::Sqlite { path } => {
                assert!(path.to_string_lossy().contains("sessions.db"));
            }
            _ => unreachable!("SQLite config"),
        }
    }

    #[test]
    fn test_parse_tool_policy() {
        // Test always_ask
        let policy = parse_tool_policy("always_ask", None).unwrap();
        assert!(matches!(policy, ToolApprovalPolicy::AlwaysAsk));

        // Test pre_approved
        let policy = parse_tool_policy("pre_approved", Some("tool1,tool2")).unwrap();
        match policy {
            ToolApprovalPolicy::PreApproved { tools } => {
                assert_eq!(tools.len(), 2);
                assert!(tools.contains("tool1"));
                assert!(tools.contains("tool2"));
            }
            _ => unreachable!("PreApproved policy"),
        }

        // Test mixed
        let policy = parse_tool_policy("mixed", Some("tool3,tool4")).unwrap();
        match policy {
            ToolApprovalPolicy::Mixed { pre_approved, .. } => {
                assert_eq!(pre_approved.len(), 2);
                assert!(pre_approved.contains("tool3"));
                assert!(pre_approved.contains("tool4"));
            }
            _ => unreachable!("Mixed policy"),
        }

        // Test invalid policy
        assert!(parse_tool_policy("invalid", None).is_err());
    }

    #[test]
    fn test_parse_metadata() {
        // Test with metadata
        let metadata = parse_metadata(Some("key1=value1,key2=value2")).unwrap();
        assert_eq!(metadata.len(), 2);
        assert_eq!(metadata.get("key1"), Some(&"value1".to_string()));
        assert_eq!(metadata.get("key2"), Some(&"value2".to_string()));

        // Test without metadata
        let metadata = parse_metadata(None).unwrap();
        assert!(metadata.is_empty());

        // Test invalid format
        assert!(parse_metadata(Some("invalid_format")).is_err());
    }
}
