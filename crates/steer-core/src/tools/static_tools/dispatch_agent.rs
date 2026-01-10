use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::agents::{
    McpAccessPolicy, agent_spec, agent_specs, agent_specs_prompt, default_agent_spec_id,
};
use crate::app::domain::runtime::RuntimeService;
use crate::config::model::builtin::claude_sonnet_4_5 as default_model;
use crate::runners::OneShotRunner;
use crate::tools::capability::Capabilities;
use crate::tools::services::{SubAgentConfig, SubAgentError};
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use crate::app::domain::event::SessionEvent;
use crate::app::domain::types::SessionId;
use crate::session::state::BackendConfig;
use crate::workspace::{
    create_workspace_from_session_config, CreateWorkspaceRequest, EnvironmentId, RepoRef, VcsStatus,
    WorkspaceCreateStrategy, WorkspaceRef,
};
use steer_tools::result::{AgentResult, AgentWorkspaceInfo, AgentWorkspaceRevision};
use steer_tools::tools::{GLOB_TOOL_NAME, GREP_TOOL_NAME, VIEW_TOOL_NAME};
use tracing::warn;
use crate::tools::ToolExecutor;

pub const DISPATCH_AGENT_TOOL_NAME: &str = "dispatch_agent";

fn dispatch_agent_description() -> String {
    let agent_specs = agent_specs_prompt();
    let agent_specs_block = if agent_specs.is_empty() {
        "No agent specs registered.".to_string()
    } else {
        agent_specs
    };

    format!(
        r#"Launch a new agent to help with a focused task. When you are searching for a keyword or file and are not confident that you will find the right match on the first try, use the Agent tool to perform the search for you.

When to use the Agent tool:
- If you are searching for a keyword like "config" or "logger", or for questions like "which file does X?", the Agent tool is strongly recommended

When NOT to use the Agent tool:
- If you want to read a specific file path, use the {} or {} tool instead of the Agent tool, to find the match more quickly
- If you are searching for a specific class definition like "class Foo", use the {} tool instead, to find the match more quickly
- If you are searching for code within a specific file or set of 2-3 files, use the {} tool instead, to find the match more quickly

Usage notes:
1. Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses
2. When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.
3. Each invocation returns a session_id. Pass it back via `mode: "resume"` to continue the conversation with the same agent.
4. When `mode` is `resume`, the session_id must refer to a child of the current session. The `agent` and `workspace` options are ignored and the existing session config is used.
5. The agent's outputs should generally be trusted
6. IMPORTANT: Only some agent specs include write tools. Use a build agent if the task requires editing files.
7. IMPORTANT: New workspaces are preserved (not auto-deleted). Clean them up manually if needed.
8. If the agent spec omits a model, the parent session's default model is used.

Workspace options:
- `workspace: "current"` to run in the current workspace
- `workspace: {{ "new": {{ "name": "..." }} }}` to run in a fresh workspace (jj only)

Session options:
- `mode: "resume"` with `session_id: "<uuid>"` to continue a prior dispatch_agent session

New session options:
- `mode: "new"` with `workspace: "current"` or `workspace: {{ "new": {{ "name": "..." }} }}`
- `agent: "<id>"` selects an agent spec (defaults to "{default_agent}")

{agent_specs_block}"#,
        VIEW_TOOL_NAME,
        GLOB_TOOL_NAME,
        GREP_TOOL_NAME,
        GREP_TOOL_NAME,
        default_agent = default_agent_spec_id(),
        agent_specs_block = agent_specs_block
    )
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceTarget {
    Current,
    New { name: String },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DispatchAgentTarget {
    New {
        workspace: WorkspaceTarget,
        #[serde(default)]
        agent: Option<String>,
    },
    Resume {
        session_id: SessionId,
    },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DispatchAgentParams {
    pub prompt: String,
    #[serde(flatten)]
    pub target: DispatchAgentTarget,
}

pub struct DispatchAgentTool;

#[async_trait]
impl StaticTool for DispatchAgentTool {
    type Params = DispatchAgentParams;
    type Output = AgentResult;

    const NAME: &'static str = DISPATCH_AGENT_TOOL_NAME;
    const DESCRIPTION: &'static str = "Launch a sub-agent to search for files or code";
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::AGENT;

    fn schema() -> steer_tools::ToolSchema {
        let settings = schemars::generate::SchemaSettings::draft07().with(|s| {
            s.inline_subschemas = true;
        });
        let schema_gen = settings.into_generator();
        let input_schema = schema_gen.into_root_schema_for::<Self::Params>();

        steer_tools::ToolSchema {
            name: Self::NAME.to_string(),
            description: dispatch_agent_description(),
            input_schema: input_schema.into(),
        }
    }

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let DispatchAgentParams { prompt, target } = params;

        let (workspace_target, agent) = match target {
            DispatchAgentTarget::Resume { session_id } => {
                return resume_agent_session(session_id, prompt, ctx).await;
            }
            DispatchAgentTarget::New { workspace, agent } => (workspace, agent),
        };

        let spawner = ctx
            .services
            .agent_spawner()
            .ok_or_else(|| StaticToolError::missing_capability("agent_spawner"))?;

        let base_workspace = ctx.services.workspace.clone();
        let base_path = base_workspace.working_directory().to_path_buf();

        let mut workspace = base_workspace.clone();
        let mut workspace_ref = None;
        let mut workspace_id = None;
        let mut workspace_name = None;
        let mut repo_id = None;
        let mut repo_ref = None;

        if let Some(manager) = ctx.services.workspace_manager() {
            if let Ok(info) = manager.resolve_workspace(&base_path).await {
                workspace_id = Some(info.workspace_id);
                workspace_name = info.name.clone();
                repo_id = Some(info.repo_id);
                workspace_ref = Some(WorkspaceRef {
                    environment_id: info.environment_id,
                    workspace_id: info.workspace_id,
                    repo_id: info.repo_id,
                });
            }
        }

        if let Some(manager) = ctx.services.repo_manager() {
            let repo_env_id = workspace_ref
                .as_ref()
                .map(|reference| reference.environment_id)
                .unwrap_or_else(EnvironmentId::local);
            if let Ok(info) = manager.resolve_repo(repo_env_id, &base_path).await {
                if repo_id.is_none() {
                    repo_id = Some(info.repo_id);
                }
                repo_ref = Some(RepoRef {
                    environment_id: info.environment_id,
                    repo_id: info.repo_id,
                    root_path: info.root_path,
                    vcs_kind: info.vcs_kind,
                });
            }
        }

        let mut new_workspace = false;
        let mut requested_workspace_name = None;

        match &workspace_target {
            WorkspaceTarget::Current => {}
            WorkspaceTarget::New { name } => {
                new_workspace = true;
                requested_workspace_name = Some(name.clone());
            }
        }

        let mut created_workspace_id = None;
        let mut status_manager = None;

        if new_workspace {
            let manager = ctx
                .services
                .workspace_manager()
                .ok_or_else(|| StaticToolError::missing_capability("workspace_manager"))?;
            status_manager = Some(manager.clone());

            let base_repo_id = repo_id.ok_or_else(|| {
                StaticToolError::execution(
                    "Current path is not a jj workspace; cannot create new workspace".to_string(),
                )
            })?;

            let create_request = CreateWorkspaceRequest {
                repo_id: base_repo_id,
                name: requested_workspace_name.clone(),
                parent_workspace_id: workspace_id,
                strategy: WorkspaceCreateStrategy::JjWorkspace,
            };

            let info = manager
                .create_workspace(create_request)
                .await
                .map_err(|e| {
                    StaticToolError::execution(format!("Failed to create workspace: {e}"))
                })?;

            workspace = manager
                .open_workspace(info.workspace_id)
                .await
                .map_err(|e| {
                    StaticToolError::execution(format!("Failed to open workspace: {e}"))
                })?;

            workspace_id = Some(info.workspace_id);
            created_workspace_id = Some(info.workspace_id);
            workspace_name = info.name.clone();
            workspace_ref = Some(WorkspaceRef {
                environment_id: info.environment_id,
                workspace_id: info.workspace_id,
                repo_id: info.repo_id,
            });

            if let Some(repo_manager) = ctx.services.repo_manager()
                && let Ok(info) = repo_manager
                    .resolve_repo(info.environment_id, workspace.working_directory())
                    .await
            {
                repo_ref = Some(RepoRef {
                    environment_id: info.environment_id,
                    repo_id: info.repo_id,
                    root_path: info.root_path,
                    vcs_kind: info.vcs_kind,
                });
            }
        }

        let env_info = workspace.environment().await.map_err(|e| {
            StaticToolError::execution(format!("Failed to get environment: {e}"))
        })?;

        let system_prompt = format!(
            r#"You are an agent for a CLI-based coding tool. Given the user's prompt, you should use the tools available to you to answer the user's question.

Notes:
1. IMPORTANT: You should be concise, direct, and to the point, since your responses will be displayed on a command line interface. Answer the user's question directly, without elaboration, explanation, or details. One word answers are best. Avoid introductions, conclusions, and explanations. You MUST avoid text before/after your response, such as "The answer is <answer>.", "Here is the content of the file..." or "Based on the information provided, the answer is..." or "Here is what I will do next...".
2. When relevant, share file names and code snippets relevant to the query
3. Any file paths you return in your final response MUST be absolute. DO NOT use relative paths.

{}
"#,
            env_info.as_context()
        );

        let agent_id = agent
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_agent_spec_id().to_string());

        let agent_spec = agent_spec(&agent_id).ok_or_else(|| {
            let available = agent_specs()
                .into_iter()
                .map(|spec| spec.id)
                .collect::<Vec<_>>()
                .join(", ");
            StaticToolError::invalid_params(format!(
                "Unknown agent spec '{agent_id}'. Available: {available}"
            ))
        })?;

        let parent_session_config = match ctx.services.event_store.load_events(ctx.session_id).await
        {
            Ok(events) => events.into_iter().find_map(|(_, event)| match event {
                SessionEvent::SessionCreated { config, .. } => Some(*config),
                _ => None,
            }),
            Err(err) => {
                warn!(
                    session_id = %ctx.session_id,
                    "Failed to load parent session config for MCP servers: {err}"
                );
                None
            }
        };

        let parent_mcp_backends = parent_session_config
            .as_ref()
            .map(|config| config.tool_config.backends.clone())
            .unwrap_or_default();

        let parent_model = parent_session_config
            .as_ref()
            .map(|config| config.default_model.clone())
            .unwrap_or_else(default_model);

        let allow_mcp_tools = agent_spec.mcp_access.allow_mcp_tools();
        let mcp_backends = match &agent_spec.mcp_access {
            McpAccessPolicy::None => Vec::new(),
            McpAccessPolicy::All => parent_mcp_backends,
            McpAccessPolicy::Allowlist(servers) => parent_mcp_backends
                .into_iter()
                .filter(|backend| match backend {
                    BackendConfig::Mcp { server_name, .. } => {
                        servers.iter().any(|allowed| allowed == server_name)
                    }
                })
                .collect(),
        };

        let config = SubAgentConfig {
            parent_session_id: ctx.session_id,
            prompt,
            allowed_tools: agent_spec.tools.clone(),
            model: agent_spec.model.clone().unwrap_or(parent_model),
            system_prompt: Some(system_prompt),
            workspace: Some(workspace),
            workspace_ref,
            workspace_id,
            repo_ref,
            workspace_name,
            mcp_backends,
            allow_mcp_tools,
        };

        let spawn_result = spawner
            .spawn(config, ctx.cancellation_token.clone())
            .await;

        let mut workspace_info = None;

        if let (Some(manager), Some(workspace_id)) = (status_manager, created_workspace_id) {
            let revision = match manager.get_workspace_status(workspace_id).await {
                Ok(status) => match status.vcs {
                    Some(vcs) => match vcs.status {
                        VcsStatus::Jj(jj_status) => jj_status.working_copy.map(|wc| {
                            AgentWorkspaceRevision {
                                vcs_kind: "jj".to_string(),
                                revision_id: wc.commit_id,
                                summary: wc.description,
                                change_id: Some(wc.change_id),
                            }
                        }),
                        VcsStatus::Git(_) => None,
                    },
                    None => None,
                },
                Err(err) => {
                    warn!(
                        workspace_id = %workspace_id.as_uuid(),
                        "Failed to get workspace status for sub-agent: {err}"
                    );
                    None
                }
            };

            workspace_info = Some(AgentWorkspaceInfo {
                workspace_id: Some(workspace_id.as_uuid().to_string()),
                revision,
            });
        }

        let result = spawn_result.map_err(|e| match e {
            SubAgentError::Cancelled => StaticToolError::Cancelled,
            other => StaticToolError::execution(other.to_string()),
        })?;

        Ok(AgentResult {
            content: result.final_message.extract_text(),
            session_id: Some(result.session_id.to_string()),
            workspace: workspace_info,
        })
    }
}

async fn resume_agent_session(
    session_id: SessionId,
    prompt: String,
    ctx: &StaticToolContext,
) -> Result<AgentResult, StaticToolError> {
    let events = ctx
        .services
        .event_store
        .load_events(session_id)
        .await
        .map_err(|e| {
            StaticToolError::execution(format!(
                "Failed to load session {session_id}: {e}"
            ))
        })?;

    let session_config = events
        .into_iter()
        .find_map(|(_, event)| match event {
            SessionEvent::SessionCreated { config, .. } => Some(*config),
            _ => None,
        })
        .ok_or_else(|| {
            StaticToolError::execution(format!(
                "Session {session_id} is missing a SessionCreated event"
            ))
        })?;

    if session_config.parent_session_id != Some(ctx.session_id) {
        return Err(StaticToolError::invalid_params(format!(
            "Session {session_id} is not a child of current session {}",
            ctx.session_id
        )));
    }

    let workspace = create_workspace_from_session_config(&session_config.workspace)
        .await
        .map_err(|e| {
            StaticToolError::execution(format!(
                "Failed to open workspace for session {session_id}: {e}"
            ))
        })?;

    let tool_executor = Arc::new(ToolExecutor::with_workspace(workspace));
    let runtime = RuntimeService::spawn(
        ctx.services.event_store.clone(),
        ctx.services.api_client.clone(),
        tool_executor,
    );

    let run_result = OneShotRunner::run_in_session_with_cancel(
        &runtime.handle,
        session_id,
        prompt,
        session_config.default_model.clone(),
        ctx.cancellation_token.clone(),
    )
    .await;

    runtime.shutdown().await;

    let run_result = run_result.map_err(|e| match e {
        crate::error::Error::Cancelled => StaticToolError::Cancelled,
        other => StaticToolError::execution(other.to_string()),
    })?;

    Ok(AgentResult {
        content: run_result.final_message.extract_text(),
        session_id: Some(run_result.session_id.to_string()),
        workspace: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Client as ApiClient;
    use crate::api::{ApiError, CompletionResponse, Provider};
    use crate::app::conversation::{AssistantContent, Message, MessageData};
    use crate::app::domain::session::EventStore;
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::app::domain::types::ToolCallId;
    use crate::agents::{AgentSpec, AgentSpecError, McpAccessPolicy, register_agent_spec};
    use crate::config::model::builtin;
    use crate::model_registry::ModelRegistry;
    use crate::session::state::{
        ApprovalRules, SessionConfig, SessionToolConfig, ToolApprovalPolicy, ToolFilter,
        ToolVisibility, UnapprovedBehavior,
    };
    use crate::tools::services::{AgentSpawner, SubAgentError, SubAgentResult, ToolServices};
    use crate::tools::McpTransport;
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex as StdMutex;
    use tokio::time::{Duration, sleep};
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    #[derive(Clone)]
    struct StubProvider {
        response: String,
    }

    impl StubProvider {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[derive(Clone)]
    struct CancelAwareProvider;

    #[async_trait]
    impl Provider for CancelAwareProvider {
        fn name(&self) -> &'static str {
            "cancel-aware"
        }

        async fn complete(
            &self,
            _model_id: &crate::config::model::ModelId,
            _messages: Vec<Message>,
            _system: Option<String>,
            _tools: Option<Vec<steer_tools::ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            token.cancelled().await;
            Err(ApiError::Cancelled {
                provider: self.name().to_string(),
            })
        }
    }

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &crate::config::model::ModelId,
            _messages: Vec<Message>,
            _system: Option<String>,
            _tools: Option<Vec<steer_tools::ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: self.response.clone(),
                }],
            })
        }
    }

    #[derive(Clone)]
    struct StubAgentSpawner {
        session_id: SessionId,
        response: String,
    }

    #[async_trait]
    impl AgentSpawner for StubAgentSpawner {
        async fn spawn(
            &self,
            _config: crate::tools::services::SubAgentConfig,
            _cancel_token: CancellationToken,
        ) -> Result<SubAgentResult, SubAgentError> {
            let timestamp = Message::current_timestamp();
            let message = Message {
                timestamp,
                id: Message::generate_id("assistant", timestamp),
                parent_message_id: None,
                data: MessageData::Assistant {
                    content: vec![AssistantContent::Text {
                        text: self.response.clone(),
                    }],
                },
            };

            Ok(SubAgentResult {
                session_id: self.session_id,
                final_message: message,
            })
        }
    }

    #[derive(Clone)]
    struct CapturingAgentSpawner {
        session_id: SessionId,
        response: String,
        captured: Arc<tokio::sync::Mutex<Option<crate::tools::services::SubAgentConfig>>>,
    }

    #[async_trait]
    impl AgentSpawner for CapturingAgentSpawner {
        async fn spawn(
            &self,
            config: crate::tools::services::SubAgentConfig,
            _cancel_token: CancellationToken,
        ) -> Result<SubAgentResult, SubAgentError> {
            let mut guard = self.captured.lock().await;
            *guard = Some(config);

            let timestamp = Message::current_timestamp();
            let message = Message {
                timestamp,
                id: Message::generate_id("assistant", timestamp),
                parent_message_id: None,
                data: MessageData::Assistant {
                    content: vec![AssistantContent::Text {
                        text: self.response.clone(),
                    }],
                },
            };

            Ok(SubAgentResult {
                session_id: self.session_id,
                final_message: message,
            })
        }
    }

    #[derive(Clone)]
    struct ToolCallThenTextProvider {
        tool_call: steer_tools::ToolCall,
        final_text: String,
        call_count: Arc<StdMutex<usize>>,
    }

    impl ToolCallThenTextProvider {
        fn new(tool_call: steer_tools::ToolCall, final_text: impl Into<String>) -> Self {
            Self {
                tool_call,
                final_text: final_text.into(),
                call_count: Arc::new(StdMutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl Provider for ToolCallThenTextProvider {
        fn name(&self) -> &'static str {
            "stub-tool-call"
        }

        async fn complete(
            &self,
            _model_id: &crate::config::model::ModelId,
            _messages: Vec<Message>,
            _system: Option<String>,
            _tools: Option<Vec<steer_tools::ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            let mut count = self
                .call_count
                .lock()
                .expect("tool call counter lock poisoned");
            let response = if *count == 0 {
                CompletionResponse {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: self.tool_call.clone(),
                    }],
                }
            } else {
                CompletionResponse {
                    content: vec![AssistantContent::Text {
                        text: self.final_text.clone(),
                    }],
                }
            };
            *count += 1;
            Ok(response)
        }
    }

    #[tokio::test]
    async fn resume_session_rejects_non_child() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let session_id = SessionId::new();
        let mut session_config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        session_config.parent_session_id = Some(parent_session_id);

        event_store.create_session(session_id).await.unwrap();
        event_store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: std::collections::HashMap::new(),
                    parent_session_id: Some(parent_session_id),
                },
            )
            .await
            .unwrap();

        let services = Arc::new(ToolServices::new(
            workspace,
            event_store,
            api_client,
        ));

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: SessionId::new(),
            cancellation_token: CancellationToken::new(),
            services,
        };

        let result = resume_agent_session(session_id, "ping".to_string(), &ctx).await;

        assert!(matches!(result, Err(StaticToolError::InvalidParams(_))));
    }

    #[tokio::test]
    async fn resume_session_accepts_child_and_returns_message() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let model = builtin::claude_sonnet_4_5();
        api_client.insert_test_provider(
            model.0.clone(),
            Arc::new(StubProvider::new("stub-response")),
        );
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let session_id = SessionId::new();
        let mut session_config = SessionConfig::read_only(model.clone());
        session_config.parent_session_id = Some(parent_session_id);

        event_store.create_session(session_id).await.unwrap();
        event_store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: std::collections::HashMap::new(),
                    parent_session_id: Some(parent_session_id),
                },
            )
            .await
            .unwrap();

        let services = Arc::new(ToolServices::new(
            workspace,
            event_store,
            api_client,
        ));

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: parent_session_id,
            cancellation_token: CancellationToken::new(),
            services,
        };

        let result = resume_agent_session(session_id, "ping".to_string(), &ctx)
            .await
            .unwrap();

        assert!(result.content.contains("stub-response"));
        assert_eq!(result.session_id.as_deref(), Some(session_id.to_string().as_str()));
    }

    #[tokio::test]
    async fn resume_session_honors_cancellation() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let model = builtin::claude_sonnet_4_5();
        api_client.insert_test_provider(model.0.clone(), Arc::new(CancelAwareProvider));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let session_id = SessionId::new();
        let mut session_config = SessionConfig::read_only(model);
        session_config.parent_session_id = Some(parent_session_id);

        event_store.create_session(session_id).await.unwrap();
        event_store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: std::collections::HashMap::new(),
                    parent_session_id: Some(parent_session_id),
                },
            )
            .await
            .unwrap();

        let services = Arc::new(ToolServices::new(
            workspace,
            event_store,
            api_client,
        ));

        let cancel_token = CancellationToken::new();
        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: parent_session_id,
            cancellation_token: cancel_token.clone(),
            services,
        };

        let cancel_task = tokio::spawn(async move {
            sleep(Duration::from_millis(10)).await;
            cancel_token.cancel();
        });

        let result = resume_agent_session(session_id, "ping".to_string(), &ctx).await;
        let _ = cancel_task.await;

        assert!(matches!(result, Err(StaticToolError::Cancelled)));
    }

    #[tokio::test]
    async fn dispatch_agent_returns_session_id() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let session_id = SessionId::new();
        let spawner = StubAgentSpawner {
            session_id,
            response: "done".to_string(),
        };

        let services = Arc::new(
            ToolServices::new(workspace, event_store, api_client)
                .with_agent_spawner(Arc::new(spawner)),
        );

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: SessionId::new(),
            cancellation_token: CancellationToken::new(),
            services,
        };

        let params = DispatchAgentParams {
            prompt: "hello".to_string(),
            target: DispatchAgentTarget::New {
                workspace: WorkspaceTarget::Current,
                agent: None,
            },
        };

        let result = DispatchAgentTool.execute(params, &ctx).await.unwrap();
        assert_eq!(result.session_id.as_deref(), Some(session_id.to_string().as_str()));
    }

    #[tokio::test]
    async fn dispatch_agent_filters_mcp_backends_by_allowlist() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let mut session_config = SessionConfig::read_only(builtin::claude_sonnet_4_5());
        session_config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "allowed-server".to_string(),
            transport: McpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port: 1111,
            },
            tool_filter: ToolFilter::All,
        });
        session_config.tool_config.backends.push(BackendConfig::Mcp {
            server_name: "blocked-server".to_string(),
            transport: McpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port: 2222,
            },
            tool_filter: ToolFilter::All,
        });

        event_store.create_session(parent_session_id).await.unwrap();
        event_store
            .append(
                parent_session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: HashMap::new(),
                    parent_session_id: None,
                },
            )
            .await
            .unwrap();

        let agent_id = format!("allowlist_{}", Uuid::new_v4());
        let spec = AgentSpec {
            id: agent_id.clone(),
            name: "allowlist test".to_string(),
            description: "allowlist test".to_string(),
            tools: vec![VIEW_TOOL_NAME.to_string()],
            mcp_access: McpAccessPolicy::Allowlist(vec!["allowed-server".to_string()]),
            model: None,
        };
        match register_agent_spec(spec) {
            Ok(()) => {}
            Err(AgentSpecError::AlreadyRegistered(_)) => {}
        }

        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let spawner = CapturingAgentSpawner {
            session_id: SessionId::new(),
            response: "ok".to_string(),
            captured: captured.clone(),
        };

        let services = Arc::new(
            ToolServices::new(workspace, event_store, api_client)
                .with_agent_spawner(Arc::new(spawner)),
        );

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: parent_session_id,
            cancellation_token: CancellationToken::new(),
            services,
        };

        let params = DispatchAgentParams {
            prompt: "test".to_string(),
            target: DispatchAgentTarget::New {
                workspace: WorkspaceTarget::Current,
                agent: Some(agent_id),
            },
        };

        let _ = DispatchAgentTool.execute(params, &ctx).await.unwrap();
        let captured = captured.lock().await.clone().expect("no config captured");

        let backend_names: Vec<String> = captured
            .mcp_backends
            .iter()
            .filter_map(|backend| match backend {
                BackendConfig::Mcp { server_name, .. } => Some(server_name.clone()),
            })
            .collect();

        assert_eq!(backend_names, vec!["allowed-server".to_string()]);
        assert!(captured.allow_mcp_tools);
    }

    #[tokio::test]
    async fn dispatch_agent_uses_parent_model_when_spec_missing_model() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let parent_model = builtin::claude_sonnet_4_5();
        let session_config = SessionConfig::read_only(parent_model.clone());

        event_store.create_session(parent_session_id).await.unwrap();
        event_store
            .append(
                parent_session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: HashMap::new(),
                    parent_session_id: None,
                },
            )
            .await
            .unwrap();

        let agent_id = format!("inherit_model_{}", Uuid::new_v4());
        let spec = AgentSpec {
            id: agent_id.clone(),
            name: "inherit model test".to_string(),
            description: "inherit model test".to_string(),
            tools: vec![VIEW_TOOL_NAME.to_string()],
            mcp_access: McpAccessPolicy::None,
            model: None,
        };
        match register_agent_spec(spec) {
            Ok(()) => {}
            Err(AgentSpecError::AlreadyRegistered(_)) => {}
        }

        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let spawner = CapturingAgentSpawner {
            session_id: SessionId::new(),
            response: "ok".to_string(),
            captured: captured.clone(),
        };

        let services = Arc::new(
            ToolServices::new(workspace, event_store, api_client)
                .with_agent_spawner(Arc::new(spawner)),
        );

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: parent_session_id,
            cancellation_token: CancellationToken::new(),
            services,
        };

        let params = DispatchAgentParams {
            prompt: "test".to_string(),
            target: DispatchAgentTarget::New {
                workspace: WorkspaceTarget::Current,
                agent: Some(agent_id),
            },
        };

        let _ = DispatchAgentTool.execute(params, &ctx).await.unwrap();
        let captured = captured.lock().await.clone().expect("no config captured");

        assert_eq!(captured.model, parent_model);
    }

    #[tokio::test]
    async fn dispatch_agent_uses_spec_model_when_set() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let parent_model = builtin::claude_sonnet_4_5();
        let session_config = SessionConfig::read_only(parent_model);

        event_store.create_session(parent_session_id).await.unwrap();
        event_store
            .append(
                parent_session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: HashMap::new(),
                    parent_session_id: None,
                },
            )
            .await
            .unwrap();

        let spec_model = builtin::claude_haiku_4_5();
        let agent_id = format!("spec_model_{}", Uuid::new_v4());
        let spec = AgentSpec {
            id: agent_id.clone(),
            name: "spec model test".to_string(),
            description: "spec model test".to_string(),
            tools: vec![VIEW_TOOL_NAME.to_string()],
            mcp_access: McpAccessPolicy::None,
            model: Some(spec_model.clone()),
        };
        match register_agent_spec(spec) {
            Ok(()) => {}
            Err(AgentSpecError::AlreadyRegistered(_)) => {}
        }

        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let spawner = CapturingAgentSpawner {
            session_id: SessionId::new(),
            response: "ok".to_string(),
            captured: captured.clone(),
        };

        let services = Arc::new(
            ToolServices::new(workspace, event_store, api_client)
                .with_agent_spawner(Arc::new(spawner)),
        );

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: parent_session_id,
            cancellation_token: CancellationToken::new(),
            services,
        };

        let params = DispatchAgentParams {
            prompt: "test".to_string(),
            target: DispatchAgentTarget::New {
                workspace: WorkspaceTarget::Current,
                agent: Some(agent_id),
            },
        };

        let _ = DispatchAgentTool.execute(params, &ctx).await.unwrap();
        let captured = captured.lock().await.clone().expect("no config captured");

        assert_eq!(captured.model, spec_model);
    }

    #[tokio::test]
    async fn resume_session_denies_disallowed_tools() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));
        let workspace = crate::workspace::create_workspace(
            &steer_workspace::WorkspaceConfig::Local {
                path: std::env::current_dir().unwrap(),
            },
        )
        .await
        .unwrap();

        let parent_session_id = SessionId::new();
        let session_id = SessionId::new();
        let model = builtin::claude_sonnet_4_5();

        let tool_call = steer_tools::ToolCall {
            name: "bash".to_string(),
            parameters: serde_json::json!({ "command": "echo denied" }),
            id: "tool_denied".to_string(),
        };
        api_client.insert_test_provider(
            model.0.clone(),
            Arc::new(ToolCallThenTextProvider::new(tool_call, "done")),
        );

        let mut tool_config = SessionToolConfig::read_only();
        tool_config.visibility = ToolVisibility::Whitelist(HashSet::from([
            VIEW_TOOL_NAME.to_string(),
        ]));
        tool_config.approval_policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Deny,
            preapproved: ApprovalRules {
                tools: HashSet::from([VIEW_TOOL_NAME.to_string()]),
                per_tool: HashMap::new(),
            },
        };

        let mut session_config = SessionConfig::read_only(model);
        session_config.tool_config = tool_config;
        session_config.parent_session_id = Some(parent_session_id);

        event_store.create_session(session_id).await.unwrap();
        event_store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    config: Box::new(session_config),
                    metadata: HashMap::new(),
                    parent_session_id: Some(parent_session_id),
                },
            )
            .await
            .unwrap();

        let services = Arc::new(ToolServices::new(
            workspace,
            event_store.clone(),
            api_client,
        ));

        let ctx = StaticToolContext {
            tool_call_id: ToolCallId::new(),
            session_id: parent_session_id,
            cancellation_token: CancellationToken::new(),
            services,
        };

        let _ = resume_agent_session(session_id, "trigger".to_string(), &ctx)
            .await
            .unwrap();

        let events = event_store.load_events(session_id).await.unwrap();
        let denied = events.iter().any(|(_, event)| match event {
            SessionEvent::ToolCallFailed { name, error, .. } => {
                name == "bash" && error.contains("denied by policy")
            }
            _ => false,
        });

        assert!(denied, "expected denied ToolCallFailed event for bash");
    }
}
