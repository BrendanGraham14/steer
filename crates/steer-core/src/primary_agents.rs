use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use crate::config::model::ModelId;
use crate::prompts::system_prompt_for_model;
use crate::session::state::{
    ApprovalRules, SessionConfig, SessionPolicyOverrides, ToolApprovalPolicy, ToolRule,
    ToolVisibility, UnapprovedBehavior,
};
use crate::tools::DISPATCH_AGENT_TOOL_NAME;
use crate::tools::static_tools::READ_ONLY_TOOL_NAMES;

pub const NORMAL_PRIMARY_AGENT_ID: &str = "normal";
pub const PLANNER_PRIMARY_AGENT_ID: &str = "planner";
pub const YOLO_PRIMARY_AGENT_ID: &str = "yolo";
pub const DEFAULT_PRIMARY_AGENT_ID: &str = NORMAL_PRIMARY_AGENT_ID;

static PLANNER_SYSTEM_PROMPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        r#"You are in planner mode. Produce a concise, step-by-step plan only.

Rules:
- Use read-only tools to gather the context you need before planning.
- When broader search is needed, use dispatch_agent with the "explore" sub-agent.
- Do not make changes or write code/patches.
- If key details are missing, ask up to three targeted questions and stop.

When you can proceed, respond using this structure (omit empty sections):
Plan:
1. ...
2. ...
3. ...

Assumptions:
- ...

Risks:
- ...

Validation:
- ...

Finish by asking the user to switch back to "{NORMAL_PRIMARY_AGENT_ID}" or "{YOLO_PRIMARY_AGENT_ID}" to execute."#,
    )
});

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

static PRIMARY_AGENT_SPECS: std::sync::LazyLock<RwLock<HashMap<String, PrimaryAgentSpec>>> =
    std::sync::LazyLock::new(|| {
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

pub fn resolve_effective_config(base_config: &SessionConfig) -> SessionConfig {
    let mut config = base_config.clone();

    let requested_agent_id = base_config
        .primary_agent_id
        .clone()
        .unwrap_or_else(|| DEFAULT_PRIMARY_AGENT_ID.to_string());

    let (primary_agent_id, spec) = if let Some(spec) = primary_agent_spec(&requested_agent_id) {
        (requested_agent_id, spec)
    } else {
        let fallback_spec =
            primary_agent_spec(DEFAULT_PRIMARY_AGENT_ID).unwrap_or_else(|| PrimaryAgentSpec {
                id: DEFAULT_PRIMARY_AGENT_ID.to_string(),
                name: "Normal".to_string(),
                description: "Default agent".to_string(),
                model: None,
                system_prompt: None,
                tool_visibility: ToolVisibility::All,
                approval_policy: ToolApprovalPolicy::default(),
            });
        (DEFAULT_PRIMARY_AGENT_ID.to_string(), fallback_spec)
    };

    let effective_model = resolve_default_model(&config, &spec, &base_config.policy_overrides);
    let effective_visibility = base_config
        .policy_overrides
        .tool_visibility
        .clone()
        .unwrap_or_else(|| spec.tool_visibility.clone());
    let effective_approval_policy = base_config
        .policy_overrides
        .approval_policy
        .apply_to(&spec.approval_policy);
    let effective_system_prompt = resolve_system_prompt(
        &config,
        &spec,
        &effective_model,
        &base_config.policy_overrides,
    );

    config.primary_agent_id = Some(primary_agent_id);
    config.default_model = effective_model;
    config.tool_config.visibility = effective_visibility;
    config.tool_config.approval_policy = effective_approval_policy;
    config.system_prompt = effective_system_prompt;

    config
}

fn resolve_default_model(
    config: &SessionConfig,
    spec: &PrimaryAgentSpec,
    overrides: &SessionPolicyOverrides,
) -> ModelId {
    let mut model = config.default_model.clone();
    if let Some(spec_model) = spec.model.as_ref() {
        model = spec_model.clone();
    }
    if let Some(override_model) = overrides.default_model.as_ref() {
        model = override_model.clone();
    }
    model
}

fn resolve_system_prompt(
    config: &SessionConfig,
    spec: &PrimaryAgentSpec,
    model: &ModelId,
    _overrides: &SessionPolicyOverrides,
) -> Option<String> {
    if let Some(prompt) = spec.system_prompt.as_ref() {
        return Some(prompt.clone());
    }

    if let Some(prompt) = config.system_prompt.as_ref()
        && !prompt.trim().is_empty()
        && !is_known_primary_agent_prompt(prompt)
    {
        return Some(prompt.clone());
    }

    Some(system_prompt_for_model(model))
}

fn is_known_primary_agent_prompt(prompt: &str) -> bool {
    primary_agent_specs()
        .iter()
        .any(|spec| spec.system_prompt.as_deref() == Some(prompt))
}

fn default_primary_agent_specs() -> Vec<PrimaryAgentSpec> {
    let planner_tool_visibility = ToolVisibility::Whitelist(
        READ_ONLY_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .chain(std::iter::once(DISPATCH_AGENT_TOOL_NAME.to_string()))
            .collect::<HashSet<_>>(),
    );

    let mut planner_approval_policy = ToolApprovalPolicy::default();
    planner_approval_policy.preapproved.per_tool.insert(
        DISPATCH_AGENT_TOOL_NAME.to_string(),
        ToolRule::DispatchAgent {
            agent_patterns: vec!["explore".to_string()],
        },
    );

    vec![
        PrimaryAgentSpec {
            id: NORMAL_PRIMARY_AGENT_ID.to_string(),
            name: "Normal".to_string(),
            description: "Default agent with full tool visibility. Tools which can write require explicit approvals."
                .to_string(),
            model: None,
            system_prompt: None,
            tool_visibility: ToolVisibility::All,
            approval_policy: ToolApprovalPolicy::default(),
        },
        PrimaryAgentSpec {
            id: PLANNER_PRIMARY_AGENT_ID.to_string(),
            name: "Planner".to_string(),
            description: "Planning-only agent with read-only tools.".to_string(),
            model: None,
            system_prompt: Some(PLANNER_SYSTEM_PROMPT.clone()),
            tool_visibility: planner_tool_visibility,
            approval_policy: planner_approval_policy,
        },
        PrimaryAgentSpec {
            id: YOLO_PRIMARY_AGENT_ID.to_string(),
            name: "Yolo".to_string(),
            description: "Full tool visibility with auto-approval for all tools.".to_string(),
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
    use crate::session::state::{
        ApprovalRulesOverrides, SessionPolicyOverrides, SessionToolConfig,
        ToolApprovalPolicyOverrides, ToolRule, ToolRuleOverrides, WorkspaceConfig,
    };
    use crate::tools::DISPATCH_AGENT_TOOL_NAME;
    use crate::tools::static_tools::READ_ONLY_TOOL_NAMES;
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
            primary_agent_id: None,
            policy_overrides: SessionPolicyOverrides::empty(),
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
    fn resolve_effective_config_preserves_base_when_unset() {
        let mut config = base_config();
        config.primary_agent_id = Some(NORMAL_PRIMARY_AGENT_ID.to_string());
        let updated = resolve_effective_config(&config);

        assert_eq!(updated.default_model, config.default_model);
        assert_eq!(updated.system_prompt, config.system_prompt);
        assert_eq!(updated.tool_config.visibility, ToolVisibility::All);
    }

    #[test]
    fn resolve_effective_config_applies_overrides() {
        let mut config = base_config();
        config.primary_agent_id = Some(NORMAL_PRIMARY_AGENT_ID.to_string());
        config.policy_overrides = SessionPolicyOverrides {
            default_model: Some(builtin::claude_haiku_4_5()),
            tool_visibility: Some(ToolVisibility::ReadOnly),
            approval_policy: ToolApprovalPolicyOverrides {
                default_behavior: Some(UnapprovedBehavior::Allow),
                preapproved: ApprovalRulesOverrides {
                    tools: ["custom_tool".to_string()].into_iter().collect(),
                    per_tool: [(
                        "bash".to_string(),
                        ToolRuleOverrides::Bash {
                            patterns: vec!["git status".to_string()],
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            },
        };

        let updated = resolve_effective_config(&config);
        assert_eq!(updated.default_model, builtin::claude_haiku_4_5());
        assert_eq!(updated.tool_config.visibility, ToolVisibility::ReadOnly);
        assert_eq!(
            updated.tool_config.approval_policy.default_behavior,
            UnapprovedBehavior::Allow
        );
        assert!(
            updated
                .tool_config
                .approval_policy
                .preapproved
                .tools
                .contains("custom_tool")
        );

        let rule = updated
            .tool_config
            .approval_policy
            .preapproved
            .per_tool
            .get("bash")
            .expect("bash rule");
        match rule {
            ToolRule::Bash { patterns } => {
                assert!(patterns.contains(&"git status".to_string()));
            }
            ToolRule::DispatchAgent { .. } => panic!("Unexpected dispatch agent rule"),
        }
    }

    #[test]
    fn planner_spec_limits_tools_and_dispatch_agent() {
        let spec = primary_agent_spec(PLANNER_PRIMARY_AGENT_ID).expect("planner spec");

        match &spec.tool_visibility {
            ToolVisibility::Whitelist(allowed) => {
                assert!(allowed.contains(DISPATCH_AGENT_TOOL_NAME));
                for name in READ_ONLY_TOOL_NAMES {
                    assert!(allowed.contains(*name));
                }
                assert_eq!(allowed.len(), READ_ONLY_TOOL_NAMES.len() + 1);
            }
            other => panic!("Unexpected tool visibility: {other:?}"),
        }

        let rule = spec
            .approval_policy
            .preapproved
            .per_tool
            .get(DISPATCH_AGENT_TOOL_NAME)
            .expect("dispatch agent rule");

        match rule {
            ToolRule::DispatchAgent { agent_patterns } => {
                assert_eq!(agent_patterns.as_slice(), ["explore"]);
            }
            ToolRule::Bash { .. } => panic!("Unexpected bash rule"),
        }
    }
}
