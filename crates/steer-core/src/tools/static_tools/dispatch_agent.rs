use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::model::builtin::claude_sonnet_4_5 as default_model;
use crate::tools::capability::Capabilities;
use crate::tools::services::SubAgentConfig;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use crate::workspace::{
    CreateWorkspaceRequest, EnvironmentId, ListWorkspacesRequest, WorkspaceCreateStrategy,
    WorkspaceId, WorkspaceRef,
};
use steer_tools::tools::{GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, VIEW_TOOL_NAME};

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
- Set `new_workspace` to true to run the sub-agent in a fresh workspace (jj only)
- Optionally set `workspace_name` to label the new workspace"#,
        format_dispatch_agent_tools(),
        VIEW_TOOL_NAME,
        GLOB_TOOL_NAME,
        GREP_TOOL_NAME,
        GREP_TOOL_NAME,
    )
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct DispatchAgentParams {
    pub prompt: String,
    #[serde(default)]
    pub new_workspace: bool,
    #[serde(default)]
    pub workspace_name: Option<String>,
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

        if let Some(manager) = ctx.services.workspace_manager() {
            if let Ok(workspaces) = manager
                .list_workspaces(ListWorkspacesRequest {
                    include_deleted: false,
                })
                .await
            {
                if let Some(info) = workspaces.into_iter().find(|info| info.path == base_path) {
                    workspace_id = Some(info.workspace_id);
                    workspace_name = info.name.clone();
                    workspace_ref = Some(WorkspaceRef {
                        environment_id: info.environment_id,
                        workspace_id: info.workspace_id,
                        path: info.path.clone(),
                        vcs_kind: info.vcs_kind,
                    });
                }
            }
        }

        if params.new_workspace {
            let manager = ctx
                .services
                .workspace_manager()
                .ok_or_else(|| StaticToolError::missing_capability("workspace_manager"))?;

            let base_ref = workspace_ref.clone().unwrap_or_else(|| WorkspaceRef {
                environment_id: EnvironmentId::local(),
                workspace_id: WorkspaceId::new(),
                path: base_path.clone(),
                vcs_kind: None,
            });

            let create_request = CreateWorkspaceRequest {
                base: Some(base_ref),
                name: params.workspace_name.clone(),
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
            workspace_name = info.name.clone();
            workspace_ref = Some(WorkspaceRef {
                environment_id: info.environment_id,
                workspace_id: info.workspace_id,
                path: info.path.clone(),
                vcs_kind: info.vcs_kind,
            });
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
            workspace_name,
        };

        let result = spawner
            .spawn(config, ctx.cancellation_token.clone())
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(DispatchAgentOutput {
            content: result.final_message.extract_text(),
        })
    }
}
