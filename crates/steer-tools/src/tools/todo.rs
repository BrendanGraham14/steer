use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::Display;

use crate::ToolSpec;
use crate::error::ToolExecutionError;
use crate::result::{TodoListResult, TodoWriteResult};

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, JsonSchema, Hash, Display)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    #[strum(serialize = "In Progress")]
    InProgress,
    Completed,
}

#[derive(
    Deserialize, Serialize, Debug, Clone, Eq, PartialEq, Ord, PartialOrd, JsonSchema, Hash, Display,
)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High = 0,
    Medium = 1,
    Low = 2,
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, JsonSchema, Hash)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
    pub id: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq, JsonSchema, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TodoWriteFileOperation {
    Created,
    Modified,
}

pub type TodoList = Vec<TodoItem>;

pub mod read {
    use super::*;
    use thiserror::Error;

    pub const TODO_READ_TOOL_NAME: &str = "read_todos";

    pub struct TodoReadToolSpec;

    impl ToolSpec for TodoReadToolSpec {
        type Params = TodoReadParams;
        type Result = TodoListResult;
        type Error = TodoReadError;

        const NAME: &'static str = TODO_READ_TOOL_NAME;
        const DISPLAY_NAME: &'static str = "Read Todos";

        fn execution_error(error: Self::Error) -> ToolExecutionError {
            ToolExecutionError::TodoRead(error)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
    #[serde(tag = "code", rename_all = "snake_case")]
    pub enum TodoReadError {
        #[error("io error: {message}")]
        Io { message: String },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
    /// This tool takes in no parameters. Leave the input blank.
    pub struct TodoReadParams {}
}

pub mod write {
    use super::*;
    use thiserror::Error;

    pub const TODO_WRITE_TOOL_NAME: &str = "write_todos";

    pub struct TodoWriteToolSpec;

    impl ToolSpec for TodoWriteToolSpec {
        type Params = TodoWriteParams;
        type Result = TodoWriteResult;
        type Error = TodoWriteError;

        const NAME: &'static str = TODO_WRITE_TOOL_NAME;
        const DISPLAY_NAME: &'static str = "Write Todos";

        fn execution_error(error: Self::Error) -> ToolExecutionError {
            ToolExecutionError::TodoWrite(error)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
    #[serde(tag = "code", rename_all = "snake_case")]
    pub enum TodoWriteError {
        #[error("io error: {message}")]
        Io { message: String },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
    pub struct TodoWriteParams {
        /// The updated todo list
        pub todos: TodoList,
    }
}
