use eyre::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use steer_core::config::model::ModelId;
use steer_core::session::{
    ApprovalRules, BackendConfig, RemoteAuth, SessionConfig, SessionToolConfig, ToolApprovalPolicy,
    ToolRule, ToolVisibility, UnapprovedBehavior, WorkspaceConfig,
};
use thiserror::Error;
use tokio::fs;
use tracing::debug;

/// Session configuration validation errors
#[derive(Debug, Error)]
pub enum SessionConfigError {
    #[error("MCP backend server_name cannot be empty")]
    EmptyServerName,

    #[error("MCP stdio transport command cannot be empty")]
    EmptyStdioCommand,

    #[error("MCP TCP transport host cannot be empty")]
    EmptyTcpHost,

    #[error("MCP TCP transport port cannot be 0")]
    InvalidTcpPort,

    #[error("MCP Unix transport path cannot be empty")]
    EmptyUnixPath,

    #[error("MCP SSE transport url cannot be empty")]
    EmptySseUrl,

    #[error("MCP HTTP transport url cannot be empty")]
    EmptyHttpUrl,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
}

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
    Local {
        #[serde(default)]
        path: Option<PathBuf>,
    },
    Remote {
        agent_address: String,
        auth: Option<RemoteAuth>,
    },
}

#[derive(Debug, Deserialize, Serialize, Default, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PartialToolConfig {
    pub backends: Option<Vec<BackendConfig>>,
    pub visibility: Option<ToolVisibilityConfig>,
    pub approvals: Option<PartialApprovalConfig>,
}

#[derive(Debug, Deserialize, Serialize, Default, JsonSchema)]
pub struct PartialApprovalConfig {
    pub default_behavior: Option<UnapprovedBehavior>,
    #[serde(default)]
    pub tools: HashSet<String>,
    pub bash: Option<PartialBashApproval>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PartialBashApproval {
    pub patterns: Vec<String>,
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

/// Overrides that can be applied from CLI arguments
#[derive(Debug, Default)]
pub struct SessionConfigOverrides {
    pub system_prompt: Option<String>,
    pub metadata: Option<String>,
}

/// Loads session configuration from files and applies overrides
pub struct SessionConfigLoader {
    default_model: ModelId,
    config_path: Option<PathBuf>,
    overrides: SessionConfigOverrides,
}

impl SessionConfigLoader {
    pub fn new(default_model: ModelId, config_path: Option<PathBuf>) -> Self {
        debug!("Loading session config from: {:?}", config_path);
        Self {
            default_model,
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
            // Discover standard session config locations (.steer/session.toml, ~/.config/steer/session.toml)
            let mut discovered: Option<SessionConfig> = None;
            for p in steer_core::utils::paths::AppPaths::discover_session_configs() {
                if let Ok(content) = fs::read_to_string(&p).await {
                    let partial: PartialSessionConfig =
                        toml::from_str(&content).with_context(|| {
                            format!("Failed to parse TOML config from: {}", p.display())
                        })?;
                    discovered = Some(self.partial_to_full(partial)?);
                    break;
                }
            }

            // Fallback to defaults if nothing discovered
            discovered.unwrap_or(SessionConfig {
                default_model: self.default_model.clone(),
                workspace: WorkspaceConfig::default(),
                workspace_ref: None,
                workspace_id: None,
                repo_ref: None,
                parent_session_id: None,
                workspace_name: None,
                tool_config: SessionToolConfig::default(),
                system_prompt: None,
                metadata: HashMap::new(),
            })
        };

        self.apply_overrides(&mut config)?;
        self.validate_config(&config)?;

        Ok(config)
    }

    fn partial_to_full(&self, partial: PartialSessionConfig) -> Result<SessionConfig> {
        let workspace = match partial.workspace {
            Some(PartialWorkspaceConfig::Local { path }) => WorkspaceConfig::Local {
                path: path.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }),
            },
            Some(PartialWorkspaceConfig::Remote {
                agent_address,
                auth,
            }) => WorkspaceConfig::Remote {
                agent_address,
                auth,
            },
            None => WorkspaceConfig::default(),
        };

        let tool_config = if let Some(partial_tool_config) = partial.tool_config {
            let backends = partial_tool_config.backends.unwrap_or_default();

            let visibility = match partial_tool_config.visibility {
                Some(ToolVisibilityConfig::String(s)) => match s.as_str() {
                    "all" => ToolVisibility::All,
                    "read_only" => ToolVisibility::ReadOnly,
                    _ => {
                        return Err(eyre::eyre!(
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

            let approval_policy = if let Some(approvals) = partial_tool_config.approvals {
                let default_behavior = approvals
                    .default_behavior
                    .unwrap_or(UnapprovedBehavior::Prompt);

                let mut per_tool = HashMap::new();
                if let Some(bash) = approvals.bash {
                    per_tool.insert(
                        "bash".to_string(),
                        ToolRule::Bash {
                            patterns: bash.patterns,
                        },
                    );
                }

                ToolApprovalPolicy {
                    default_behavior,
                    preapproved: ApprovalRules {
                        tools: approvals.tools,
                        per_tool,
                    },
                }
            } else {
                ToolApprovalPolicy::default()
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

        debug!("Loaded tool config: {:?}", tool_config);

        Ok(SessionConfig {
            default_model: self.default_model.clone(),
            workspace,
            workspace_ref: None,
            workspace_id: None,
            repo_ref: None,
            parent_session_id: None,
            workspace_name: None,
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
            let metadata = steer_core::utils::session::parse_metadata(Some(metadata_str))?;
            config.metadata.extend(metadata);
        }

        Ok(())
    }

    fn validate_config(&self, config: &SessionConfig) -> Result<(), SessionConfigError> {
        for backend in &config.tool_config.backends {
            let BackendConfig::Mcp {
                server_name,
                transport,
                ..
            } = backend;

            if server_name.is_empty() {
                return Err(SessionConfigError::EmptyServerName);
            }

            match transport {
                steer_core::tools::McpTransport::Stdio { command, .. } => {
                    if command.is_empty() {
                        return Err(SessionConfigError::EmptyStdioCommand);
                    }
                    if which::which(command).is_err() {
                        tracing::warn!(
                            "MCP command '{}' for server '{}' not found in PATH",
                            command,
                            server_name
                        );
                    }
                }
                steer_core::tools::McpTransport::Tcp { host, port } => {
                    if host.is_empty() {
                        return Err(SessionConfigError::EmptyTcpHost);
                    }
                    if *port == 0 {
                        return Err(SessionConfigError::InvalidTcpPort);
                    }
                }
                #[cfg(unix)]
                steer_core::tools::McpTransport::Unix { path } => {
                    if path.is_empty() {
                        return Err(SessionConfigError::EmptyUnixPath);
                    }
                }
                steer_core::tools::McpTransport::Sse { url, .. } => {
                    if url.is_empty() {
                        return Err(SessionConfigError::EmptySseUrl);
                    }
                }
                steer_core::tools::McpTransport::Http { url, .. } => {
                    if url.is_empty() {
                        return Err(SessionConfigError::EmptyHttpUrl);
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_core::config::provider::ProviderId;
    use steer_core::session::ToolFilter;

    fn test_model() -> ModelId {
        (
            ProviderId("test-provider".to_string()),
            "test-model".to_string(),
        )
    }

    #[tokio::test]
    async fn test_backend_serialization() {
        // Test that we can serialize and deserialize BackendConfig
        let backend = BackendConfig::Mcp {
            server_name: "test".to_string(),
            transport: steer_core::tools::McpTransport::Stdio {
                command: "python".to_string(),
                args: vec!["-m".to_string(), "test".to_string()],
            },
            tool_filter: ToolFilter::All,
        };

        let json = serde_json::to_string(&backend).unwrap();
        println!("Backend JSON: {json}");

        let backend2: BackendConfig = serde_json::from_str(&json).unwrap();
        match backend2 {
            BackendConfig::Mcp {
                server_name,
                transport,
                ..
            } => {
                assert_eq!(server_name, "test");
                match transport {
                    steer_core::tools::McpTransport::Stdio { command, args } => {
                        assert_eq!(command, "python");
                        assert_eq!(args, vec!["-m", "test"]);
                    }
                    _ => unreachable!("Expected Stdio transport"),
                }
            }
            _ => unreachable!("Expected correct variant"),
        }
    }

    #[tokio::test]
    async fn test_partial_config_parsing() {
        // Test simple config without backends
        let toml_content = r#"
[tool_config]
visibility = "all"
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
        let loader = SessionConfigLoader::new(test_model(), None);
        let config = loader.load().await.unwrap();

        // Should get defaults
        assert!(matches!(config.workspace, WorkspaceConfig::Local { .. }));
        // Default policy is Prompt for unapproved with empty preapproved set
        assert!(matches!(
            config.tool_config.approval_policy.default_behavior,
            UnapprovedBehavior::Prompt
        ));
    }

    #[tokio::test]
    async fn test_config_loader_with_overrides() {
        let overrides = SessionConfigOverrides {
            system_prompt: Some("Custom prompt".to_string()),
            metadata: Some("key1=value1,key2=value2".to_string()),
        };

        let loader = SessionConfigLoader::new(test_model(), None).with_overrides(overrides);
        let config = loader.load().await.unwrap();

        assert_eq!(config.system_prompt, Some("Custom prompt".to_string()));
        assert_eq!(config.metadata.get("key1"), Some(&"value1".to_string()));
    }

    #[tokio::test]
    async fn test_load_non_existent_file() {
        let loader = SessionConfigLoader::new(
            test_model(),
            Some(PathBuf::from("/tmp/non_existent_file.toml")),
        );
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

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
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

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid visibility string"));
        assert!(err.to_string().contains("Expected 'all' or 'read_only'"));
    }

    #[tokio::test]
    async fn test_invalid_default_behavior_config() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config.approvals]
default_behavior = "invalid_value"
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(
            result.is_err(),
            "Should fail to parse invalid default_behavior"
        );
    }

    #[tokio::test]
    async fn test_mcp_backend_validation_empty_server_name() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
[tool_config]
backends = [
  {{ type = "mcp", server_name = "", transport = {{ type = "stdio", command = "python", args = ["-m", "test"] }}, tool_filter = "all" }}
]
"#).unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
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
  {{ type = "mcp", server_name = "test", transport = {{ type = "stdio", command = "", args = ["-m", "test"] }}, tool_filter = "all" }}
]
"#).unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let result = loader.load().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("MCP stdio transport command cannot be empty")
        );
    }

    #[tokio::test]
    async fn test_file_config_with_cli_overrides() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
system_prompt = "Original prompt"

[tool_config]
visibility = "all"

[tool_config.approvals]
default_behavior = "prompt"

[metadata]
key1 = "original1"
key2 = "original2"
"#
        )
        .unwrap();

        let overrides = SessionConfigOverrides {
            system_prompt: Some("Overridden prompt".to_string()),
            metadata: Some("key2=overridden2,key3=new3".to_string()),
        };

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()))
            .with_overrides(overrides);
        let config = loader.load().await.unwrap();

        assert_eq!(config.system_prompt, Some("Overridden prompt".to_string()));
        assert_eq!(config.metadata.get("key1"), Some(&"original1".to_string()));
        assert_eq!(
            config.metadata.get("key2"),
            Some(&"overridden2".to_string())
        );
        assert_eq!(config.metadata.get("key3"), Some(&"new3".to_string()));

        assert!(matches!(config.tool_config.visibility, ToolVisibility::All));
        assert!(matches!(
            config.tool_config.approval_policy.default_behavior,
            UnapprovedBehavior::Prompt
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

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        match &config.tool_config.visibility {
            ToolVisibility::Whitelist(tools) => {
                assert_eq!(tools.len(), 3);
                assert!(tools.contains("grep"));
                assert!(tools.contains("ls"));
                assert!(tools.contains("view"));
            }
            _ => unreachable!("Expected Whitelist visibility"),
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

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        match &config.tool_config.visibility {
            ToolVisibility::Blacklist(tools) => {
                assert_eq!(tools.len(), 2);
                assert!(tools.contains("bash"));
                assert!(tools.contains("edit_file"));
            }
            _ => unreachable!("Expected Blacklist visibility"),
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

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
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
                    _ => unreachable!("Expected Bearer auth"),
                }
            }
            _ => unreachable!("Expected Remote workspace"),
        }
    }

    #[tokio::test]
    async fn test_bash_approval_patterns() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config.approvals.bash]
patterns = [
    "git status",
    "git log*",
    "npm run*",
    "cargo build*"
]
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        let bash_rule = config
            .tool_config
            .approval_policy
            .preapproved
            .per_tool
            .get("bash");
        assert!(bash_rule.is_some(), "Bash rule should be present");

        match bash_rule.unwrap() {
            ToolRule::Bash { patterns } => {
                assert_eq!(patterns.len(), 4);
                assert_eq!(patterns[0], "git status");
                assert_eq!(patterns[1], "git log*");
                assert_eq!(patterns[2], "npm run*");
                assert_eq!(patterns[3], "cargo build*");
            }
        }
    }

    #[tokio::test]
    async fn test_bash_approval_empty_patterns() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config.approvals.bash]
patterns = []
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        let bash_rule = config
            .tool_config
            .approval_policy
            .preapproved
            .per_tool
            .get("bash");
        assert!(bash_rule.is_some());

        match bash_rule.unwrap() {
            ToolRule::Bash { patterns } => {
                assert_eq!(patterns.len(), 0);
            }
        }
    }

    #[tokio::test]
    async fn test_bash_approval_with_other_settings() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config]
visibility = "all"

[tool_config.approvals]
default_behavior = "prompt"

[tool_config.approvals.bash]
patterns = ["ls -la", "pwd"]
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        assert!(matches!(config.tool_config.visibility, ToolVisibility::All));
        assert!(matches!(
            config.tool_config.approval_policy.default_behavior,
            UnapprovedBehavior::Prompt
        ));

        let bash_rule = config
            .tool_config
            .approval_policy
            .preapproved
            .per_tool
            .get("bash");
        assert!(bash_rule.is_some());

        match bash_rule.unwrap() {
            ToolRule::Bash { patterns } => {
                assert_eq!(patterns.len(), 2);
                assert_eq!(patterns[0], "ls -la");
                assert_eq!(patterns[1], "pwd");
            }
        }
    }

    #[tokio::test]
    async fn test_approvals_without_bash_patterns() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[tool_config.approvals]
tools = ["grep", "ls"]
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        assert!(
            config
                .tool_config
                .approval_policy
                .preapproved
                .tools
                .contains("grep")
        );
        assert!(
            config
                .tool_config
                .approval_policy
                .preapproved
                .tools
                .contains("ls")
        );
        assert!(
            config
                .tool_config
                .approval_policy
                .preapproved
                .per_tool
                .get("bash")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_full_config_with_approvals() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
system_prompt = "You are a helpful assistant"

[workspace]
type = "local"

[tool_config]
visibility = "all"
backends = []

[tool_config.approvals]
default_behavior = "prompt"
tools = ["grep", "ls", "view"]

[tool_config.approvals.bash]
patterns = [
    "git status",
    "git diff",
    "git log --oneline",
    "npm test",
    "cargo check"
]

[metadata]
project = "test-project"
"#
        )
        .unwrap();

        let loader = SessionConfigLoader::new(test_model(), Some(temp_file.path().to_path_buf()));
        let config = loader.load().await.unwrap();

        assert_eq!(
            config.system_prompt,
            Some("You are a helpful assistant".to_string())
        );
        assert!(matches!(config.workspace, WorkspaceConfig::Local { .. }));
        assert_eq!(
            config.metadata.get("project"),
            Some(&"test-project".to_string())
        );

        let policy = &config.tool_config.approval_policy;
        assert!(matches!(
            policy.default_behavior,
            UnapprovedBehavior::Prompt
        ));
        assert_eq!(policy.preapproved.tools.len(), 3);
        assert!(policy.preapproved.tools.contains("grep"));
        assert!(policy.preapproved.tools.contains("ls"));
        assert!(policy.preapproved.tools.contains("view"));

        let bash_rule = policy.preapproved.per_tool.get("bash");
        assert!(bash_rule.is_some());

        match bash_rule.unwrap() {
            ToolRule::Bash { patterns } => {
                assert_eq!(patterns.len(), 5);
                assert_eq!(patterns[0], "git status");
                assert_eq!(patterns[1], "git diff");
                assert_eq!(patterns[2], "git log --oneline");
                assert_eq!(patterns[3], "npm test");
                assert_eq!(patterns[4], "cargo check");
            }
        }
    }
}
