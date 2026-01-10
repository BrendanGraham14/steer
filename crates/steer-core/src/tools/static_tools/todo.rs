use async_trait::async_trait;
use std::fs;
use std::path::PathBuf;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::{TodoListResult, TodoWriteResult};
use steer_tools::tools::TODO_READ_TOOL_NAME;
use steer_tools::tools::TODO_WRITE_TOOL_NAME;
use steer_tools::tools::todo::{TodoItem, TodoWriteFileOperation};
use steer_tools::tools::todo::read::TodoReadParams;
use steer_tools::tools::todo::write::TodoWriteParams;

pub struct TodoReadTool;

#[async_trait]
impl StaticTool for TodoReadTool {
    type Params = TodoReadParams;
    type Output = TodoListResult;

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
        if ctx.is_cancelled() {
            return Err(StaticToolError::Cancelled);
        }

        let todos = read_todos().map_err(|e| StaticToolError::Io(e.to_string()))?;
        Ok(TodoListResult { todos })
    }
}

pub struct TodoWriteTool;

#[async_trait]
impl StaticTool for TodoWriteTool {
    type Params = TodoWriteParams;
    type Output = TodoWriteResult;

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
        if ctx.is_cancelled() {
            return Err(StaticToolError::Cancelled);
        }

        let operation =
            write_todos(&params.todos).map_err(|e| StaticToolError::Io(e.to_string()))?;

        Ok(TodoWriteResult {
            todos: params.todos,
            operation,
        })
    }
}

fn get_todos_dir() -> Result<PathBuf, std::io::Error> {
    let home_dir = dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    let todos_dir = home_dir.join(".steer").join("todos");

    if !todos_dir.exists() {
        fs::create_dir_all(&todos_dir)?;
    }

    Ok(todos_dir)
}

fn get_todo_file_path() -> Result<PathBuf, std::io::Error> {
    let workspace_id = match std::env::var("STEER_WORKSPACE_ID") {
        Ok(id) => id,
        Err(_) => {
            let current_dir = std::env::current_dir()?;
            hex::encode(current_dir.to_string_lossy().as_bytes())
        }
    };
    let dir = get_todos_dir()?;
    Ok(dir.join(format!("{workspace_id}.json")))
}

fn read_todos() -> Result<Vec<TodoItem>, std::io::Error> {
    let file_path = get_todo_file_path()?;

    if !file_path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&file_path)?;
    serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn write_todos(todos: &[TodoItem]) -> Result<TodoWriteFileOperation, std::io::Error> {
    let file_path = get_todo_file_path()?;
    let file_existed = file_path.exists();

    let content = serde_json::to_string_pretty(todos)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&file_path, content)?;

    Ok(if file_existed {
        TodoWriteFileOperation::Modified
    } else {
        TodoWriteFileOperation::Created
    })
}
