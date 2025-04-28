use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::api::InputSchema;
use crate::tools::error::ToolError;

#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// A unique, stable identifier for the tool (e.g., "Bash", "GrepTool").
    fn name(&self) -> &'static str;

    /// A description of what the tool does.
    fn description(&self) -> &'static str;

    /// The JSON schema defining the tool's expected input parameters.
    fn input_schema(&self) -> &'static InputSchema;

    /// Executes the tool with the given parameters and cancellation token.
    ///
    /// # Arguments
    /// * `parameters` - The parameters for the tool call, matching the `input_schema`.
    /// * `token` - An optional cancellation token to signal interruption.
    ///
    /// # Returns
    /// A `Result` containing the string output of the tool on success,
    /// or a `ToolError` on failure.
    async fn execute(
        &self,
        parameters: Value, // Will be deserialized within the impl
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError>;
}
