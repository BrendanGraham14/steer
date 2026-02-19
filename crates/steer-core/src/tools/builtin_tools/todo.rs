use async_trait::async_trait;

use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use steer_tools::result::{TodoListResult, TodoWriteResult};
use steer_tools::tools::todo::TodoWriteFileOperation;
use steer_tools::tools::todo::read::{TodoReadError, TodoReadParams, TodoReadToolSpec};
use steer_tools::tools::todo::write::{TodoWriteError, TodoWriteParams, TodoWriteToolSpec};

const TODO_READ_DESCRIPTION: &str = r#"Use this tool to read the current to-do list for the session. This tool should be used proactively and frequently to ensure that you are aware of
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

const TODO_WRITE_DESCRIPTION: &str = r"Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.

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
- Only have ONE task in_progress at any time";

pub struct TodoReadTool;

#[async_trait]
impl BuiltinTool for TodoReadTool {
    type Params = TodoReadParams;
    type Output = TodoListResult;
    type Spec = TodoReadToolSpec;

    const DESCRIPTION: &'static str = TODO_READ_DESCRIPTION;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        _params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<TodoReadError>> {
        if ctx.is_cancelled() {
            return Err(BuiltinToolError::Cancelled);
        }

        let todos = ctx
            .services
            .event_store
            .load_todos(ctx.session_id)
            .await
            .map_err(|e| {
                BuiltinToolError::execution(TodoReadError::Io {
                    message: e.to_string(),
                })
            })?
            .unwrap_or_default();

        Ok(TodoListResult { todos })
    }
}

pub struct TodoWriteTool;

#[async_trait]
impl BuiltinTool for TodoWriteTool {
    type Params = TodoWriteParams;
    type Output = TodoWriteResult;
    type Spec = TodoWriteToolSpec;

    const DESCRIPTION: &'static str = TODO_WRITE_DESCRIPTION;
    const REQUIRES_APPROVAL: bool = false;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &BuiltinToolContext,
    ) -> Result<Self::Output, BuiltinToolError<TodoWriteError>> {
        if ctx.is_cancelled() {
            return Err(BuiltinToolError::Cancelled);
        }

        let existing = ctx
            .services
            .event_store
            .load_todos(ctx.session_id)
            .await
            .map_err(|e| {
                BuiltinToolError::execution(TodoWriteError::Io {
                    message: e.to_string(),
                })
            })?;

        ctx.services
            .event_store
            .save_todos(ctx.session_id, &params.todos)
            .await
            .map_err(|e| {
                BuiltinToolError::execution(TodoWriteError::Io {
                    message: e.to_string(),
                })
            })?;

        let operation = if existing.is_some() {
            TodoWriteFileOperation::Modified
        } else {
            TodoWriteFileOperation::Created
        };

        Ok(TodoWriteResult {
            todos: params.todos,
            operation,
        })
    }
}
