use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::model::builtin::claude_sonnet_4_5 as default_model;
use crate::tools::capability::Capabilities;
use crate::tools::services::SubAgentConfig;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use crate::workspace::{
    CreateWorkspaceRequest, DeleteWorkspaceRequest, EnvironmentId, RepoRef,
    WorkspaceCreateStrategy, WorkspaceRef,
};
use steer_tools::tools::{GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME};
use tracing::warn;

pub const DISPATCH_AGENT_TOOL_NAME: &str = "dispatch_agent";

const DISPATCH_AGENT_TOOLS: [&str; 4] =
    [GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME];

fn format_dispatch_agent_tools() -> String {
    DISPATCH_AGENT_TOOLS
        .iter()
        .map(|tool| tool.to_string())
        .collect::<Vec<String>>()
        .join(", ")
}

fn dispatch_agent_description() -> String {
    format!(
        r#"Launch a new agent that has access to the following tools: {}. When you are searching for a keyword or file and are not confident that you will find the right match on the first try, use the Agent tool to perform the search for you.

When to use the Agent tool:
- If you are searching for a keyword like "config" or "logger", or for questions like "which file does X?", the Agent tool is strongly recommended

When NOT to use the Agent tool:
- If you want to read a specific file path, use the {} or {} tool instead of the Agent tool, to find the match more quickly
- If you are searching for a specific class definition like "class Foo", use the {} tool instead, to find the match more quickly
- If you are searching for code within a specific file or set of 2-3 files, use the {} tool instead, to find the match more quickly

Usage notes:
1. Launch multiple agents concurrently whenever possible, to maximize performance; to do that, use a single message with multiple tool uses
2. When the agent is done, it will return a single message back to you. The result returned by the agent is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.
3. Each agent invocation is stateless. You will not be able to send additional messages to the agent, nor will the agent be able to communicate with you outside of its final report. Therefore, your prompt should contain a highly detailed task description for the agent to perform autonomously and you should specify exactly what information the agent should return back to you in its final and only message to you.
4. The agent's outputs should generally be trusted
5. IMPORTANT: The agent can not modify files. If you want to modify files, do it directly instead of going through the agent.

Workspace options:
- `workspace: "current"` to run in the current workspace
- `workspace: {{ "new": {{ "name": "..." }} }}` to run in a fresh workspace (jj only)"#,
        format_dispatch_agent_tools(),
        VIEW_TOOL_NAME,
        GLOB_TOOL_NAME,
        GREP_TOOL_NAME,
        GREP_TOOL_NAME,
    )
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceTarget {
    Current,
    New { name: String },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DispatchAgentParams {
    pub prompt: String,
    pub workspace: WorkspaceTarget,
}

#[derive(Debug, Serialize)]
pub struct DispatchAgentOutput {
    pub content: String,
}

pub struct DispatchAgentTool;

#[async_trait]
impl StaticTool for DispatchAgentTool {
    type Params = DispatchAgentParams;
    type Output = DispatchAgentOutput;

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

        match &params.workspace {
            WorkspaceTarget::Current => {}
            WorkspaceTarget::New { name } => {
                new_workspace = true;
                requested_workspace_name = Some(name.clone());
            }
        }

        let mut created_workspace_id = None;
        let mut cleanup_manager = None;

        if new_workspace {
            let manager = ctx
                .services
                .workspace_manager()
                .ok_or_else(|| StaticToolError::missing_capability("workspace_manager"))?;
            cleanup_manager = Some(manager.clone());

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

        let config = SubAgentConfig {
            parent_session_id: ctx.session_id,
            prompt: params.prompt,
            allowed_tools: DISPATCH_AGENT_TOOLS.iter().map(|s| s.to_string()).collect(),
            model: default_model(),
            system_prompt: Some(system_prompt),
            workspace: Some(workspace),
            workspace_ref,
            workspace_id,
            repo_ref,
            workspace_name,
        };

        let spawn_result = spawner
            .spawn(config, ctx.cancellation_token.clone())
            .await;

        if let (Some(manager), Some(workspace_id)) = (cleanup_manager, created_workspace_id) {
            if let Err(err) = manager
                .delete_workspace(DeleteWorkspaceRequest { workspace_id })
                .await
            {
                warn!(
                    workspace_id = %workspace_id.as_uuid(),
                    "Failed to cleanup sub-agent workspace: {err}"
                );
            }
        }

        let result = spawn_result
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(DispatchAgentOutput {
            content: result.final_message.extract_text(),
        })
    }
}
