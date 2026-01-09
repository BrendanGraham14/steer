use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const BASH_TOOL_NAME: &str = "bash";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BashParams {
    /// The command to execute
    pub command: String,
    /// Optional timeout in milliseconds (default 3600000, max 3600000)
    #[schemars(range(min = 1, max = 3600000))]
    pub timeout: Option<u64>,
}
