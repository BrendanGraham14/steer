use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::config::model::ModelId;
use crate::session::state::{
    ApprovalRules, SessionConfig, ToolApprovalPolicy, ToolVisibility, UnapprovedBehavior,
};

pub const DEFAULT_PRIMARY_AGENT_ID: &str = "normal";

const PLANNER_SYSTEM_PROMPT: &str = "You are in planner mode. Provide a concise, step-by-step \
plan only. Do not call tools or make changes. When ready to execute, ask the user to switch \
back to 'normal' or 'yolo'.";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrimaryAgentSpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub model: Option<ModelId>,
    pub system_prompt: Option<String>,
    pub tool_visibility: ToolVisibility,
    pub approval_policy: ToolApprovalPolicy,
}

static PRIMARY_AGENT_SPECS: Lazy<RwLock<HashMap<String, PrimaryAgentSpec>>> = Lazy::new(|| {
    let mut specs = HashMap::new();
    for spec in default_primary_agent_specs() {
        specs.insert(spec.id.clone(), spec);
    }
    RwLock::new(specs)
});

pub fn primary_agent_spec(id: &str) -> Option<PrimaryAgentSpec> {
    let registry = PRIMARY_AGENT_SPECS.read().ok()?;
    registry.get(id).cloned()
}

pub fn primary_agent_specs() -> Vec<PrimaryAgentSpec> {
    let registry = match PRIMARY_AGENT_SPECS.read() {
        Ok(registry) => registry,
        Err(_) => return Vec::new(),
    };
    let mut specs: Vec<_> = registry.values().cloned().collect();
    specs.sort_by(|a, b| a.id.cmp(&b.id));
    specs
}

pub fn default_primary_agent_id() -> &'static str {
    DEFAULT_PRIMARY_AGENT_ID
}

pub fn apply_primary_agent_to_config(
    spec: &PrimaryAgentSpec,
    base_config: &SessionConfig,
) -> SessionConfig {
    let mut config = base_config.clone();

    if let Some(model) = spec.model.clone() {
        config.default_model = model;
    }

    if let Some(prompt) = spec.system_prompt.as_ref() {
        config.system_prompt = Some(prompt.clone());
    }

    config.tool_config.visibility = spec.tool_visibility.clone();
    config.tool_config.approval_policy = spec.approval_policy.clone();

    config
}

fn default_primary_agent_specs() -> Vec<PrimaryAgentSpec> {
    vec![
        PrimaryAgentSpec {
            id: "normal".to_string(),
            name: "Normal".to_string(),
            description: "Default mode with full tools and approvals.".to_string(),
            model: None,
            system_prompt: None,
            tool_visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::default(),
        },
        PrimaryAgentSpec {
            id: "planner".to_string(),
            name: "Planner".to_string(),
            description: "Planning-only mode with read-only tools.".to_string(),
            model: None,
            system_prompt: Some(PLANNER_SYSTEM_PROMPT.to_string()),
            tool_visibility: ToolVisibility::ReadOnly,
            approval_policy: ToolApprovalPolicy::default(),
        },
        PrimaryAgentSpec {
            id: "yolo".to_string(),
            name: "Yolo".to_string(),
            description: "Full tools with auto-approval.".to_string(),
            model: None,
            system_prompt: None,
            tool_visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy {
                default_behavior: UnapprovedBehavior::Allow,
                preapproved: ApprovalRules::default(),
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::builtin;
    use crate::session::state::{SessionToolConfig, WorkspaceConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn base_config() -> SessionConfig {
        SessionConfig {
            workspace: WorkspaceConfig::Local {
                path: PathBuf::from("/tmp"),
            },
            workspace_ref: None,
            workspace_id: None,
            repo_ref: None,
            parent_session_id: None,
            workspace_name: None,
            tool_config: SessionToolConfig::read_only(),
            system_prompt: Some("base prompt".to_string()),
            metadata: HashMap::new(),
            default_model: builtin::claude_sonnet_4_5(),
        }
    }

    #[test]
    fn default_primary_agent_exists() {
        let id = default_primary_agent_id();
        let spec = primary_agent_spec(id);
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().id, id);
    }

    #[test]
    fn apply_primary_agent_preserves_base_when_unset() {
        let config = base_config();
        let spec = primary_agent_spec("normal").expect("normal spec");
        let updated = apply_primary_agent_to_config(&spec, &config);

        assert_eq!(updated.default_model, config.default_model);
        assert_eq!(updated.system_prompt, config.system_prompt);
        assert_eq!(updated.tool_config.visibility, ToolVisibility::All);
    }

    #[test]
    fn apply_primary_agent_overrides_fields() {
        let config = base_config();
        let spec = PrimaryAgentSpec {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test spec".to_string(),
            model: Some(builtin::claude_sonnet_4_5()),
            system_prompt: Some("override prompt".to_string()),
            tool_visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy {
                default_behavior: UnapprovedBehavior::Allow,
                preapproved: ApprovalRules::default(),
            },
        };

        let updated = apply_primary_agent_to_config(&spec, &config);
        assert_eq!(updated.system_prompt, Some("override prompt".to_string()));
        assert_eq!(updated.tool_config.approval_policy, spec.approval_policy);
    }
}
