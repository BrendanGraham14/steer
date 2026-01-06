use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::Tool;
use steer_tools::result::{TodoListResult, TodoWriteResult};
use steer_tools::tools::todo::read::TodoReadParams;
use steer_tools::tools::todo::write::TodoWriteParams;
use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus};

use super::to_tools_context;

pub const TODO_READ_TOOL_NAME: &str = "TodoRead";
pub const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TodoReadToolParams {}

#[derive(Debug, Serialize)]
pub struct TodoReadToolOutput {
    pub todos: Vec<TodoItemOutput>,
}

#[derive(Debug, Serialize)]
pub struct TodoItemOutput {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
}

impl From<TodoListResult> for TodoReadToolOutput {
    fn from(r: TodoListResult) -> Self {
        Self {
            todos: r
                .todos
                .into_iter()
                .map(|t| TodoItemOutput {
                    content: t.content,
                    status: t.status.to_string(),
                    priority: t.priority.to_string(),
                    id: t.id,
                })
                .collect(),
        }
    }
}

pub struct TodoReadTool;

#[async_trait]
impl StaticTool for TodoReadTool {
    type Params = TodoReadToolParams;
    type Output = TodoReadToolOutput;

    const NAME: &'static str = TODO_READ_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Use this tool to read the current to-do list for the session. This tool should be used proactively and frequently to ensure that you are aware of
the status of the current task list. You should make use of this tool as often as possible, especially in the following situations:
- At the beginning of conversations to see what's pending
- Before starting new tasks to prioritize work
- When the user asks about previous tasks or plans
- Whenever you're uncertain about what to do next
- After completing tasks to update your understanding of remaining work
- After every few messages to ensure you're on track

Usage:
- This tool takes in no parameters. So leave the input blank or empty. DO NOT include a dummy object, placeholder string or a key like "input" or "empty". LEAVE IT BLANK.
- Returns a list of todo items with their status, priority, and content
- Use this information to track progress and plan next steps
- If no todos exist yet, an empty list will be returned"#;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        _params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let read_params = TodoReadParams {};

        let params_json = serde_json::to_value(read_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::TodoReadTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result.into())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TodoWriteToolParams {
    pub todos: Vec<TodoItemInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TodoItemInput {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct TodoWriteToolOutput {
    pub todos: Vec<TodoItemOutput>,
    pub operation: String,
}

impl From<TodoWriteResult> for TodoWriteToolOutput {
    fn from(r: TodoWriteResult) -> Self {
        Self {
            todos: r
                .todos
                .into_iter()
                .map(|t| TodoItemOutput {
                    content: t.content,
                    status: t.status.to_string(),
                    priority: t.priority.to_string(),
                    id: t.id,
                })
                .collect(),
            operation: format!("{:?}", r.operation),
        }
    }
}

pub struct TodoWriteTool;

#[async_trait]
impl StaticTool for TodoWriteTool {
    type Params = TodoWriteToolParams;
    type Output = TodoWriteToolOutput;

    const NAME: &'static str = TODO_WRITE_TOOL_NAME;
    const DESCRIPTION: &'static str = r#"Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.

When to Use This Tool:
1. Complex multi-step tasks - When a task requires 3 or more distinct steps
2. Non-trivial tasks - Tasks requiring careful planning
3. User explicitly requests todo list
4. User provides multiple tasks
5. After receiving new instructions
6. After completing a task - Mark it complete

When NOT to Use This Tool:
1. Single, straightforward tasks
2. Trivial tasks
3. Tasks completed in less than 3 steps
4. Purely conversational requests

Task States:
- pending: Task not yet started
- in_progress: Currently working on (limit to ONE at a time)
- completed: Task finished successfully

Task Management:
- Update status in real-time
- Mark tasks complete IMMEDIATELY after finishing
- Only have ONE task in_progress at any time"#;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        let tools_ctx = to_tools_context(ctx);

        let todos: Vec<TodoItem> = params
            .todos
            .into_iter()
            .map(|t| TodoItem {
                content: t.content,
                status: match t.status.to_lowercase().as_str() {
                    "in_progress" => TodoStatus::InProgress,
                    "completed" => TodoStatus::Completed,
                    _ => TodoStatus::Pending,
                },
                priority: match t.priority.to_lowercase().as_str() {
                    "high" => TodoPriority::High,
                    "low" => TodoPriority::Low,
                    _ => TodoPriority::Medium,
                },
                id: t.id,
            })
            .collect();

        let write_params = TodoWriteParams { todos };

        let params_json = serde_json::to_value(write_params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        let tool = steer_tools::tools::TodoWriteTool;
        let result = tool
            .execute(params_json, &tools_ctx)
            .await
            .map_err(|e| StaticToolError::execution(e.to_string()))?;

        Ok(result.into())
    }
}
