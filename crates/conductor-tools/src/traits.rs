use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use crate::context::ExecutionContext;
use crate::error::ToolError;
use crate::result::{ToolOutput, ToolResult as ToolResultEnum};
use crate::schema::InputSchema;

/// Type alias for tool execution results
pub type ToolResult<T> = Result<T, ToolError>;

#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// The concrete output type for this tool
    type Output: ToolOutput + Serialize;

    /// A unique, stable identifier for the tool (e.g., "bash", "edit_file").
    fn name(&self) -> &'static str;

    /// A description of what the tool does.
    fn description(&self) -> String;

    /// The JSON schema defining the tool's expected input parameters.
    fn input_schema(&self) -> &'static InputSchema;

    /// Executes the tool with the given parameters and execution context.
    ///
    /// # Arguments
    /// * `parameters` - The parameters for the tool call, matching the `input_schema`.
    /// * `context` - Execution context containing cancellation token, working directory, etc.
    ///
    /// # Returns
    /// A `ToolResult` containing the typed output on success, or a `ToolError` on failure.
    async fn execute(
        &self,
        parameters: Value,
        context: &ExecutionContext,
    ) -> ToolResult<Self::Output>;

    /// Indicates if this tool requires user approval before execution.
    ///
    /// Tools that modify the filesystem or external state should return true.
    /// Default implementation returns true (requiring approval).
    /// Tools should override this to return false if they only read data.
    fn requires_approval(&self) -> bool {
        true
    }
}

#[async_trait]
pub trait ExecutableTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> String;
    fn input_schema(&self) -> &'static InputSchema;
    async fn run(&self, params: Value, ctx: &ExecutionContext)
    -> Result<ToolResultEnum, ToolError>;
    fn requires_approval(&self) -> bool;
}

#[async_trait]
impl<T> ExecutableTool for T
where
    T: Tool + Send + Sync,
    T::Output: Into<ToolResultEnum> + Send,
{
    fn name(&self) -> &'static str {
        Tool::name(self)
    }

    fn description(&self) -> String {
        Tool::description(self)
    }

    fn input_schema(&self) -> &'static InputSchema {
        Tool::input_schema(self)
    }

    async fn run(&self, p: Value, c: &ExecutionContext) -> Result<ToolResultEnum, ToolError> {
        Ok(Tool::execute(self, p, c).await?.into())
    }

    fn requires_approval(&self) -> bool {
        Tool::requires_approval(self)
    }
}
