use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use thiserror::Error;

use crate::config::model::ModelId;
use steer_tools::tools::edit::multi_edit::MULTI_EDIT_TOOL_NAME;
use steer_tools::tools::replace::REPLACE_TOOL_NAME;
use steer_tools::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME,
};

pub const DEFAULT_AGENT_SPEC_ID: &str = "explore";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpAccessPolicy {
    None,
    Allowlist(Vec<String>),
    All,
}

impl McpAccessPolicy {
    pub fn allows_server(&self, server_name: &str) -> bool {
        match self {
            McpAccessPolicy::None => false,
            McpAccessPolicy::All => true,
            McpAccessPolicy::Allowlist(servers) => servers.iter().any(|s| s == server_name),
        }
    }

    pub fn allow_mcp_tools(&self) -> bool {
        !matches!(self, McpAccessPolicy::None)
    }

    pub fn describe(&self) -> String {
        match self {
            McpAccessPolicy::None => "none".to_string(),
            McpAccessPolicy::All => "all".to_string(),
            McpAccessPolicy::Allowlist(servers) => {
                let list = if servers.is_empty() {
                    "<empty>".to_string()
                } else {
                    servers.join(", ")
                };
                format!("allowlist({list})")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub mcp_access: McpAccessPolicy,
    #[serde(default)]
    pub model: Option<ModelId>,
}

#[derive(Debug, Error)]
pub enum AgentSpecError {
    #[error("Agent spec already registered: {0}")]
    AlreadyRegistered(String),
    #[error("Agent spec registry lock poisoned")]
    RegistryPoisoned,
}

static AGENT_SPECS: std::sync::LazyLock<RwLock<HashMap<String, AgentSpec>>> =
    std::sync::LazyLock::new(|| {
        let mut specs = HashMap::new();
        for spec in default_agent_specs() {
            specs.insert(spec.id.clone(), spec);
        }
        RwLock::new(specs)
    });

pub fn register_agent_spec(spec: AgentSpec) -> Result<(), AgentSpecError> {
    let mut registry = AGENT_SPECS
        .write()
        .map_err(|_| AgentSpecError::RegistryPoisoned)?;
    if registry.contains_key(&spec.id) {
        return Err(AgentSpecError::AlreadyRegistered(spec.id));
    }
    registry.insert(spec.id.clone(), spec);
    Ok(())
}

pub fn agent_spec(id: &str) -> Option<AgentSpec> {
    let registry = AGENT_SPECS.read().ok()?;
    registry.get(id).cloned()
}

pub fn agent_specs() -> Vec<AgentSpec> {
    let registry = match AGENT_SPECS.read() {
        Ok(registry) => registry,
        Err(_) => return Vec::new(),
    };
    let mut specs: Vec<_> = registry.values().cloned().collect();
    specs.sort_by(|a, b| a.id.cmp(&b.id));
    specs
}

pub fn default_agent_spec_id() -> &'static str {
    DEFAULT_AGENT_SPEC_ID
}

pub fn agent_specs_prompt() -> String {
    let specs = agent_specs();
    if specs.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    lines.push("Available sub-agent specs:".to_string());
    for spec in specs {
        let tools = spec.tools.join(", ");
        let mcp = spec.mcp_access.describe();
        let model = spec
            .model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider.storage_key(), model.id));
        let mut details = format!("tools: {tools}; mcp: {mcp}");
        if let Some(model) = model {
            details.push_str(&format!("; model: {model}"));
        }
        lines.push(format!("- {}: {} ({details})", spec.id, spec.description));
    }
    lines.join("\n")
}

fn default_agent_specs() -> Vec<AgentSpec> {
    let explore_tools = vec![GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME]
        .into_iter()
        .map(|tool| tool.to_string())
        .collect();

    let build_tools = vec![
        GLOB_TOOL_NAME,
        GREP_TOOL_NAME,
        LS_TOOL_NAME,
        VIEW_TOOL_NAME,
        EDIT_TOOL_NAME,
        MULTI_EDIT_TOOL_NAME,
        REPLACE_TOOL_NAME,
        BASH_TOOL_NAME,
    ]
    .into_iter()
    .map(|tool| tool.to_string())
    .collect();

    vec![
        AgentSpec {
            id: "explore".to_string(),
            name: "Explore agent".to_string(),
            description: "Read-only search and inspection".to_string(),
            tools: explore_tools,
            mcp_access: McpAccessPolicy::None,
            model: None,
        },
        AgentSpec {
            id: "build".to_string(),
            name: "Build agent".to_string(),
            description: "Read/write changes plus build commands".to_string(),
            tools: build_tools,
            mcp_access: McpAccessPolicy::All,
            model: None,
        },
    ]
}
