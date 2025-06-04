use async_trait::async_trait;
use serde_json::Value;

use crate::context::ExecutionContext;
use crate::error::ToolError;
use crate::schema::InputSchema;

#[async_trait]
pub trait Tool: Send + Sync + 'static {
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
    /// A `Result` containing the string output of the tool on success,
    /// or a `ToolError` on failure.
    async fn execute(
        &self,
        parameters: Value,
        context: &ExecutionContext,
    ) -> Result<String, ToolError>;

    /// Indicates if this tool requires user approval before execution.
    ///
    /// Tools that modify the filesystem or external state should return true.
    /// Default implementation returns true (requiring approval).
    /// Tools should override this to return false if they only read data.
    fn requires_approval(&self) -> bool {
        true
    }
}
