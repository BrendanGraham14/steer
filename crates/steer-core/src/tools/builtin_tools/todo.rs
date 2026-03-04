use async_trait::async_trait;

use crate::tools::builtin_tool::{BuiltinTool, BuiltinToolContext, BuiltinToolError};
use crate::tools::capability::Capabilities;
use steer_tools::result::{TodoListResult, TodoWriteResult};
use steer_tools::tools::todo::TodoWriteFileOperation;
use steer_tools::tools::todo::read::{TodoReadError, TodoReadParams, TodoReadToolSpec};
use steer_tools::tools::todo::write::{TodoWriteError, TodoWriteParams, TodoWriteToolSpec};

const TODO_READ_DESCRIPTION: &str = r"Use this tool to read the current session todo list when task tracking is relevant.

When this tool is helpful:
- At the start of complex or multi-step tasks (to see if a list already exists)
- Before giving progress updates or status summaries
- After completing todo items to verify what remains
- When the user asks about plans, priorities, or previous tasks

When this tool is usually unnecessary:
- Simple one-step tasks
- Purely conversational requests
- Repeated polling when no task state has changed

Usage:
- This tool takes no parameters.
- Returns todo items with status, priority, content, and id.
- If no todos exist yet, it returns an empty list.";

const TODO_WRITE_DESCRIPTION: &str = r"Use this tool to create or update a structured task list when it adds clear value for the current coding session.

When to use this tool:
1. Complex tasks with multiple meaningful steps
2. Work that benefits from explicit ordering or checkpoints
3. Cases where the user asks for a plan or todo tracking
4. Long-running tasks where periodic progress updates are useful

When not to use this tool:
1. Single, straightforward tasks
2. Trivial or purely conversational requests
3. Work that will be completed in one short response
4. Frequent micro-updates that do not change task state

Task states:
- pending: Task not yet started
- in_progress: Task currently being worked on (prefer one active item)
- completed: Task finished successfully

Task management:
- Keep tasks concise and outcome-oriented
- Update statuses when progress meaningfully changes
- Mark tasks completed once done and keep the list tidy";

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
