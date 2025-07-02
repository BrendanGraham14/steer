use anyhow::{Context, Result};
use conductor_core::session::{
    BackendConfig, ContainerRuntime, RemoteAuth, SessionConfig, SessionToolConfig,
    ToolApprovalPolicy, ToolVisibility, WorkspaceConfig,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::fs;

/// Partial session configuration that can be loaded from a TOML file.
/// All fields are optional so users can specify only what they want to override.
#[derive(Debug, Deserialize, Serialize, Default, JsonSchema)]
pub struct PartialSessionConfig {
    #[schemars(description = "URL to the JSON schema file")]
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    pub workspace: Option<PartialWorkspaceConfig>,
    pub tool_config: Option<PartialToolConfig>,
    pub system_prompt: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartialWorkspaceConfig {
    Local,
    Remote {
        agent_address: String,
        auth: Option<RemoteAuth>,
    },
    Container {
        image: String,
        runtime: ContainerRuntime,
    },
}

#[derive(Debug, Deserialize, Serialize, Default, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PartialToolConfig {
    pub backends: Option<Vec<BackendConfig>>,
    pub visibility: Option<ToolVisibilityConfig>,
    pub approval_policy: Option<ToolApprovalPolicyConfig>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ToolVisibilityConfig {
    String(String), // "all" or "read_only"
    Object(ToolVisibilityObject),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolVisibilityObject {
    Whitelist(HashSet<String>),
    Blacklist(HashSet<String>),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ToolApprovalPolicyConfig {
    String(String),             // "always_ask"
    Tagged(ToolApprovalPolicy), // Direct deserialization for tagged enum format
}

/// Overrides that can be applied from CLI arguments
#[derive(Debug, Default)]
pub struct SessionConfigOverrides {
    pub system_prompt: Option<String>,
    pub metadata: Option<String>,
}

/// Loads session configuration from files and applies overrides
pub struct SessionConfigLoader {
    config_path: Option<PathBuf>,
    overrides: SessionConfigOverrides,
}

impl SessionConfigLoader {
    pub fn new(config_path: Option<PathBuf>) -> Self {
        Self {
            config_path,
            overrides: SessionConfigOverrides::default(),
        }
    }

    pub fn with_overrides(mut self, overrides: SessionConfigOverrides) -> Self {
        self.overrides = overrides;
        self
    }

    pub async fn load(&self) -> Result<SessionConfig> {
        let mut config = if let Some(path) = &self.config_path {
            // Load from TOML file
            let content = fs::read_to_string(path)
                .await
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;

            let partial: PartialSessionConfig = toml::from_str(&content)
                .with_context(|| format!("Failed to parse TOML config from: {}", path.display()))?;

            self.partial_to_full(partial)?
        } else {
            // Use defaults
            SessionConfig {
                workspace: WorkspaceConfig::default(),
                tool_config: SessionToolConfig::default(),
                system_prompt: None,
                metadata: HashMap::new(),
            }
        };

        // Apply CLI overrides
        self.apply_overrides(&mut config)?;

        // Validate the final config
        self.validate_config(&config)?;

        Ok(config)
    }

    fn partial_to_full(&self, partial: PartialSessionConfig) -> Result<SessionConfig> {
        let workspace = match partial.workspace {
            Some(PartialWorkspaceConfig::Local) => WorkspaceConfig::Local,
            Some(PartialWorkspaceConfig::Remote {
                agent_address,
                auth,
            }) => WorkspaceConfig::Remote {
                agent_address,
                auth,
            },
            Some(PartialWorkspaceConfig::Container { image, runtime }) => {
                WorkspaceConfig::Container { image, runtime }
            }
            None => WorkspaceConfig::default(),
        };

        let tool_config = if let Some(partial_tool_config) = partial.tool_config {
            let backends = partial_tool_config.backends.unwrap_or_default();

            let visibility = match partial_tool_config.visibility {
                Some(ToolVisibilityConfig::String(s)) => match s.as_str() {
                    "all" => ToolVisibility::All,
                    "read_only" => ToolVisibility::ReadOnly,
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Invalid visibility string: {}. Expected 'all' or 'read_only'",
                            s
                        ));
                    }
                },
                Some(ToolVisibilityConfig::Object(obj)) => match obj {
                    ToolVisibilityObject::Whitelist(tools) => ToolVisibility::Whitelist(tools),
                    ToolVisibilityObject::Blacklist(tools) => ToolVisibility::Blacklist(tools),
                },
                None => ToolVisibility::default(),
            };

            let approval_policy = match partial_tool_config.approval_policy {
                Some(ToolApprovalPolicyConfig::String(s)) => match s.as_str() {
                    "always_ask" => ToolApprovalPolicy::AlwaysAsk,
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Invalid approval policy string: {}. Expected 'always_ask'",
                            s
                        ));
                    }
                },
                Some(ToolApprovalPolicyConfig::Tagged(policy)) => policy,
                None => ToolApprovalPolicy::AlwaysAsk,
            };

            SessionToolConfig {
                backends,
                visibility,
                approval_policy,
                metadata: HashMap::new(),
            }
        } else {
            SessionToolConfig::default()
        };

        Ok(SessionConfig {
            workspace,
            tool_config,
            system_prompt: partial.system_prompt,
            metadata: partial.metadata.unwrap_or_default(),
        })
    }

    fn apply_overrides(&self, config: &mut SessionConfig) -> Result<()> {
        // Apply system prompt override
        if let Some(system_prompt) = &self.overrides.system_prompt {
            config.system_prompt = Some(system_prompt.clone());
        }

        // Apply metadata overrides
        if let Some(metadata_str) = &self.overrides.metadata {
            let metadata = conductor_core::utils::session::parse_metadata(Some(metadata_str))?;
            config.metadata.extend(metadata);
        }

        Ok(())
    }

    fn validate_config(&self, config: &SessionConfig) -> Result<()> {
        // Validate MCP backends have required fields
        for backend in &config.tool_config.backends {
            if let BackendConfig::Mcp {
                server_name,
                command,
                ..
            } = backend
            {
                if server_name.is_empty() {
                    return Err(anyhow::anyhow!("MCP backend server_name cannot be empty"));
                }
                if command.is_empty() {
                    return Err(anyhow::anyhow!("MCP backend command cannot be empty"));
                }

                // Check if command exists in PATH
                if which::which(command).is_err() {
                    // Log warning but don't fail - the command might be a full path or available later
                    tracing::warn!(
                        "MCP command '{}' for server '{}' not found in PATH",
                        command,
                        server_name
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::session::ToolFilter;

    #[tokio::test]
    async fn test_backend_serialization() {
        // Test that we can serialize and deserialize BackendConfig
        let backend = BackendConfig::Mcp {
            server_name: "test".to_string(),
            transport: "stdio".to_string(),
            command: "python".to_string(),
            args: vec!["-m".to_string(), "test".to_string()],
            tool_filter: ToolFilter::All,
        };

        let json = serde_json::to_string(&backend).unwrap();
        println!("Backend JSON: {}", json);

        let backend2: BackendConfig = serde_json::from_str(&json).unwrap();
        match backend2 {
            BackendConfig::Mcp { server_name, .. } => {
                assert_eq!(server_name, "test");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[tokio::test]
    async fn test_partial_config_parsing() {
        // Test simple config without backends
        let toml_content = r#"
[tool_config]
visibility = "all"
approval_policy = "always_ask"
"#;

        let partial: PartialSessionConfig = toml::from_str(toml_content).unwrap();
        assert!(partial.tool_config.is_some());
    }

    #[tokio::test]
    async fn test_config_with_empty_backends() {
        // Test config with empty backends array
        let toml_content = r#"
[tool_config]
backends = []
visibility = "all"
"#;

        let partial: PartialSessionConfig = toml::from_str(toml_content).unwrap();
        assert!(partial.tool_config.is_some());

        let tool_config = partial.tool_config.unwrap();
        assert!(tool_config.backends.is_some());
        assert_eq!(tool_config.backends.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_full_config_parsing() {
        let toml_content = r#"
system_prompt = "You are a helpful assistant."

[workspace]
type = "local"

[tool_config]
visibility = "read_only"

[metadata]
project = "my-project"
"#;

        let partial: PartialSessionConfig = toml::from_str(toml_content).unwrap();
        assert!(partial.workspace.is_some());
        assert!(partial.system_prompt.is_some());
        assert_eq!(
            partial.system_prompt.unwrap(),
            "You are a helpful assistant."
        );
        assert!(partial.metadata.is_some());
    }

    #[tokio::test]
    async fn test_config_loader() {
        let loader = SessionConfigLoader::new(None);
        let config = loader.load().await.unwrap();

        // Should get defaults
        assert!(matches!(config.workspace, WorkspaceConfig::Local));
        assert!(matches!(
            config.tool_config.approval_policy,
            ToolApprovalPolicy::AlwaysAsk
        ));
    }

    #[tokio::test]
    async fn test_config_loader_with_overrides() {
        let overrides = SessionConfigOverrides {
            system_prompt: Some("Custom prompt".to_string()),
            metadata: Some("key1=value1,key2=value2".to_string()),
        };

        let loader = SessionConfigLoader::new(None).with_overrides(overrides);
        let config = loader.load().await.unwrap();

        assert_eq!(config.system_prompt, Some("Custom prompt".to_string()));
        assert_eq!(config.metadata.get("key1"), Some(&"value1".to_string()));
    }

    #[tokio::test]
    async fn test_load_non_existent_file() {
        let loader = SessionConfigLoader::new(Some(PathBuf::from("/tmp/non_existent_file.toml")));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to read config file"));
    }

    #[tokio::test]
    async fn test_load_invalid_toml() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "invalid toml syntax {{").unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to parse TOML config"));
    }

    #[tokio::test]
    async fn test_invalid_visibility_config() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config]
visibility = "invalid_value"
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid visibility string"));
        assert!(err.to_string().contains("Expected 'all' or 'read_only'"));
    }

    #[tokio::test]
    async fn test_invalid_approval_policy_config() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config]
approval_policy = "invalid_policy"
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid approval policy string"));
        assert!(err.to_string().contains("Expected 'always_ask'"));
    }

    #[tokio::test]
    async fn test_mcp_backend_validation_empty_server_name() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
[tool_config]
backends = [
  {{ type = "mcp", server_name = "", transport = "stdio", command = "python", args = ["-m", "test"], tool_filter = "all" }}
]
"#).unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("MCP backend server_name cannot be empty")
        );
    }

    #[tokio::test]
    async fn test_mcp_backend_validation_empty_command() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
[tool_config]
backends = [
  {{ type = "mcp", server_name = "test", transport = "stdio", command = "", args = ["-m", "test"], tool_filter = "all" }}
]
"#).unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("MCP backend command cannot be empty")
        );
    }

    #[tokio::test]
    async fn test_file_config_with_cli_overrides() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a config file with initial values
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
system_prompt = "Original prompt"

[tool_config]
visibility = "all"
approval_policy = "always_ask"

[metadata]
key1 = "original1"
key2 = "original2"
"#
        )
        .unwrap();

        // Apply CLI overrides
        let overrides = SessionConfigOverrides {
            system_prompt: Some("Overridden prompt".to_string()),
            metadata: Some("key2=overridden2,key3=new3".to_string()),
        };

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()))
            .with_overrides(overrides);
        let config = loader.load().await.unwrap();

        // Check that overrides were applied
        assert_eq!(config.system_prompt, Some("Overridden prompt".to_string()));

        // Check metadata was merged (key1 unchanged, key2 overridden, key3 added)
        assert_eq!(config.metadata.get("key1"), Some(&"original1".to_string()));
        assert_eq!(
            config.metadata.get("key2"),
            Some(&"overridden2".to_string())
        );
        assert_eq!(config.metadata.get("key3"), Some(&"new3".to_string()));

        // Visibility and approval policy should remain from file
        assert!(matches!(config.tool_config.visibility, ToolVisibility::All));
        assert!(matches!(
            config.tool_config.approval_policy,
            ToolApprovalPolicy::AlwaysAsk
        ));
    }

    #[tokio::test]
    async fn test_complex_tool_visibility_whitelist() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config]
visibility = {{ whitelist = ["grep", "ls", "view"] }}
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        match &config.tool_config.visibility {
            ToolVisibility::Whitelist(tools) => {
                assert_eq!(tools.len(), 3);
                assert!(tools.contains("grep"));
                assert!(tools.contains("ls"));
                assert!(tools.contains("view"));
            }
            _ => panic!("Expected Whitelist visibility"),
        }
    }

    #[tokio::test]
    async fn test_complex_tool_visibility_blacklist() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config]
visibility = {{ blacklist = ["bash", "edit_file"] }}
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        match &config.tool_config.visibility {
            ToolVisibility::Blacklist(tools) => {
                assert_eq!(tools.len(), 2);
                assert!(tools.contains("bash"));
                assert!(tools.contains("edit_file"));
            }
            _ => panic!("Expected Blacklist visibility"),
        }
    }

    #[tokio::test]
    async fn test_workspace_remote_config() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[workspace]
type = "remote"
agent_address = "192.168.1.100:50051"
auth = {{ Bearer = {{ token = "secret-token" }} }}
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        match &config.workspace {
            WorkspaceConfig::Remote {
                agent_address,
                auth,
            } => {
                assert_eq!(agent_address, "192.168.1.100:50051");
                assert!(auth.is_some());
                match auth.as_ref().unwrap() {
                    RemoteAuth::Bearer { token } => {
                        assert_eq!(token, "secret-token");
                    }
                    _ => panic!("Expected Bearer auth"),
                }
            }
            _ => panic!("Expected Remote workspace"),
        }
    }
}
