use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::fs;

use std::sync::Arc;

use crate::session::stores::sqlite::SqliteSessionStore;
use crate::session::{
    Session,
    state::{SessionConfig, SessionToolConfig, ToolApprovalPolicy, WorkspaceConfig},
    store::SessionStore,
};

pub fn create_session_store_path() -> Result<std::path::PathBuf> {
    let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("Could not determine home directory"))?;
    let db_path = home_dir.join(".coder").join("sessions.db");
    Ok(db_path)
}

pub async fn create_session_store() -> Result<Arc<dyn SessionStore>> {
    // Create SQLite session store in user's home directory
    let db_path = create_session_store_path()?;

    // Create directory if it doesn't exist
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create sessions directory: {}", e))?;
    }

    let store = SqliteSessionStore::new(&db_path)
        .await
        .map_err(|e| anyhow!("Failed to create session store: {}", e))?;

    Ok(Arc::new(store))
}

pub fn create_default_session_config() -> SessionConfig {
    SessionConfig {
        workspace: WorkspaceConfig::default(),
        tool_policy: ToolApprovalPolicy::AlwaysAsk,
        tool_config: SessionToolConfig::default(),
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
                return Err(anyhow!(
                    "pre_approved_tools is required when using pre_approved policy"
                ));
            };
            Ok(ToolApprovalPolicy::PreApproved(tools))
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
        _ => Err(anyhow!(
            "Invalid tool policy: {}. Valid options: always_ask, pre_approved, mixed",
            policy_str
        )),
    }
}

pub fn parse_metadata(metadata_str: Option<&str>) -> Result<HashMap<String, String>> {
    let mut metadata = HashMap::new();

    if let Some(meta_str) = metadata_str {
        for pair in meta_str.split(',') {
            let parts: Vec<&str> = pair.split('=').collect();
            if parts.len() != 2 {
                return Err(anyhow!(
                    "Invalid metadata format. Expected key=value pairs separated by commas"
                ));
            }
            metadata.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
    }

    Ok(metadata)
}

pub fn create_mock_session(id: &str, tool_policy: ToolApprovalPolicy) -> Session {
    let config = SessionConfig {
        workspace: WorkspaceConfig::default(),
        tool_policy,
        tool_config: SessionToolConfig::default(),
        metadata: std::collections::HashMap::new(),
    };
    Session::new(id.to_string(), config)
}
