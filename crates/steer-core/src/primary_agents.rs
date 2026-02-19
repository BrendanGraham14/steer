use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use crate::agents::DEFAULT_AGENT_SPEC_ID;
use crate::config::model::ModelId;
use crate::prompts::system_prompt_for_model;
use crate::session::state::{
    ApprovalRules, SessionConfig, SessionPolicyOverrides, ToolApprovalPolicy, ToolRule,
    ToolVisibility, UnapprovedBehavior,
};
use crate::tools::DISPATCH_AGENT_TOOL_NAME;
use crate::tools::static_tools::READ_ONLY_TOOL_NAMES;

pub const NORMAL_PRIMARY_AGENT_ID: &str = "normal";
pub const PLANNER_PRIMARY_AGENT_ID: &str = "plan";
pub const YOLO_PRIMARY_AGENT_ID: &str = "yolo";
pub const DEFAULT_PRIMARY_AGENT_ID: &str = NORMAL_PRIMARY_AGENT_ID;

static PLANNER_SYSTEM_PROMPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        r#"You are in plan mode. Your job is to either ask clarifying questions or provide an execution-ready plan.

Core rules:
- Use read-only tools to gather the context you need before planning.
- When broader search is needed, use dispatch_agent with the "explore" sub-agent.
- Do not make changes or write code/patches.
- First check whether scope, constraints, or success criteria are missing.
- If missing details would materially change the plan, ask targeted clarifying questions and stop.
- Ask the minimum set of high-signal questions needed to unblock planning.

Planning workflow:
1) Gather context with read-only tools before writing any plan.
2) If decision-critical unknowns remain, ask clarifying questions and stop.
3) Only emit a plan once you can name concrete files, interfaces, or validation checks.

Plan step rules:
- Do not use generic discovery as Step 1 ("explore codebase", "identify files", "investigate implementation") if it can be resolved during planning.
- The first plan step should usually be an execution action (edit/create/refactor/test), not reconnaissance.
- Exception: include one bounded "Investigation spike" step only when uncertainty cannot be resolved during planning (for example, runtime-only behavior). Include explicit exit criteria.
- Before finalizing, if Step 1 is exploration and no hard blocker exists, continue exploring now and rewrite the plan.

Output protocol:
- If multiple materially different plans are plausible, ask clarifying questions before choosing one.
- Do not provide a plan in the same response as clarifying questions.
- Prefer clarifying questions over speculative assumptions.
- Choose the structure that best fits the task.
- Archetypes below are examples/templates, not an exhaustive taxonomy.
- If no archetype fits cleanly, use a custom structure tailored to the task.
- Keep plans right-sized: 2-4 steps for small tasks, 4-7 steps for medium tasks, phased breakdown for large tasks.
- Include concrete file paths, interfaces, and validation commands whenever possible.
- When presenting options, recommend one path and state why. Leave options open only when the decision requires user input.
- Do not include empty sections.

Plan structure archetypes (examples/templates):

1) RCA Bugfix (for defects/regressions)
Problem:
Root Cause:
Solution:
Files to Modify:
- path: change
Verification:
1. ...
2. ...

2) Scoped Feature (for bounded feature work)
Goal:
Current State:
Implementation Steps:
1. ...
2. ...
Files to Modify/Create:
- path: change
Verification:
1. ...
2. ...

3) Multi-Phase Program (for broad, cross-cutting efforts)
Context:
Implementation Order:
Phase 1: ...
Phase 2: ...
Dependencies & Risks:
- ...
Validation Strategy:
- ...

4) Review Findings (for code/design reviews)
Findings (highest severity first):
1. [High] file:line - issue and impact
2. [Medium] ...
Fix Plan:
1. ...
Validation:
- ...
Open Questions:
- ... (optional)

5) Refactoring / Migration (for restructuring or migrating code)
Goal:
Current → Target:
Migration Steps:
1. ...
2. ...
- Prefer direct cutover; avoid backwards-compatibility shims unless required by external consumers.
Files to Modify:
- path: change
Verification:
1. ...
2. ...

6) Design Exploration (for architecture/tradeoff questions)
Decision to Make:
Options:
- Option A: pros/cons
- Option B: pros/cons
Recommendation:
- chosen option + rationale
Open Questions:
- ... (optional)
Validation / Spike:
- ...

When details are missing, or structure choice is ambiguous, respond using:
Questions:
1. ...
2. ...

Global quality bar for every plan:
- Steps must be actionable and testable.
- Verification must include specific checks (commands, tests, or observable outcomes).
- Mention assumptions only when required and keep them concrete.
- Mention risks only when concrete and non-generic.

Execution note:
- Plan mode cannot execute changes.
- If execution is needed, mention that switching to "{NORMAL_PRIMARY_AGENT_ID}" or "{YOLO_PRIMARY_AGENT_ID}" is required."#,
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

fn approval_policy_with_dispatch_explore_preapproval() -> ToolApprovalPolicy {
    let mut policy = ToolApprovalPolicy::default();
    policy.preapproved.per_tool.insert(
        DISPATCH_AGENT_TOOL_NAME.to_string(),
        ToolRule::DispatchAgent {
            agent_patterns: vec![DEFAULT_AGENT_SPEC_ID.to_string()],
        },
    );
    policy
}

fn default_primary_agent_specs() -> Vec<PrimaryAgentSpec> {
    let planner_tool_visibility = ToolVisibility::Whitelist(
        READ_ONLY_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .chain(std::iter::once(DISPATCH_AGENT_TOOL_NAME.to_string()))
            .collect::<HashSet<_>>(),
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
            approval_policy: approval_policy_with_dispatch_explore_preapproval(),
        },
        PrimaryAgentSpec {
            id: PLANNER_PRIMARY_AGENT_ID.to_string(),
            name: "Plan".to_string(),
            description: "Plan-only agent with read-only tools.".to_string(),
            model: None,
            system_prompt: Some(PLANNER_SYSTEM_PROMPT.clone()),
            tool_visibility: planner_tool_visibility,
            approval_policy: approval_policy_with_dispatch_explore_preapproval(),
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
        ToolApprovalPolicyOverrides, ToolDecision, ToolRule, ToolRuleOverrides, WorkspaceConfig,
    };
    use crate::tools::DISPATCH_AGENT_TOOL_NAME;
    use crate::tools::static_tools::READ_ONLY_TOOL_NAMES;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use steer_tools::tools::{TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME};

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
            auto_compaction: crate::session::state::AutoCompactionConfig::default(),
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
            UnapprovedBehavior::Prompt
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
    fn normal_spec_preapproves_dispatch_agent_for_explore() {
        let spec = primary_agent_spec(NORMAL_PRIMARY_AGENT_ID).expect("normal spec");

        let rule = spec
            .approval_policy
            .preapproved
            .per_tool
            .get(DISPATCH_AGENT_TOOL_NAME)
            .expect("dispatch agent rule");

        match rule {
            ToolRule::DispatchAgent { agent_patterns } => {
                assert_eq!(agent_patterns.as_slice(), [DEFAULT_AGENT_SPEC_ID]);
            }
            ToolRule::Bash { .. } => panic!("Unexpected bash rule"),
        }
    }

    #[test]
    fn plan_spec_limits_tools_and_dispatch_agent() {
        let spec = primary_agent_spec(PLANNER_PRIMARY_AGENT_ID).expect("plan spec");

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
                assert_eq!(agent_patterns.as_slice(), [DEFAULT_AGENT_SPEC_ID]);
            }
            ToolRule::Bash { .. } => panic!("Unexpected bash rule"),
        }
    }

    #[test]
    fn all_primary_agents_allow_todos_by_default() {
        for agent_id in [
            NORMAL_PRIMARY_AGENT_ID,
            PLANNER_PRIMARY_AGENT_ID,
            YOLO_PRIMARY_AGENT_ID,
        ] {
            let spec = primary_agent_spec(agent_id).expect("primary agent spec");
            assert_eq!(
                spec.approval_policy.tool_decision(TODO_READ_TOOL_NAME),
                ToolDecision::Allow
            );
            assert_eq!(
                spec.approval_policy.tool_decision(TODO_WRITE_TOOL_NAME),
                ToolDecision::Allow
            );
        }
    }
}
