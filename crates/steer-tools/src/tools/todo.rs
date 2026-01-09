use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::Display;

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

#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema, Hash)]
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

    pub const TODO_READ_TOOL_NAME: &str = "TodoRead";

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
    /// This tool takes in no parameters. Leave the input blank.
    pub struct TodoReadParams {}
}

pub mod write {
    use super::*;

    pub const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

    #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
    pub struct TodoWriteParams {
        /// The updated todo list
        pub todos: TodoList,
    }
}
