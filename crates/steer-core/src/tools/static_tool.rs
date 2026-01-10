use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use crate::app::domain::types::{SessionId, ToolCallId};
use steer_tools::error::ToolExecutionError;
use steer_tools::result::ToolResult;
use steer_tools::ToolSchema;

use super::capability::Capabilities;
use super::services::ToolServices;

#[derive(Debug, Clone)]
pub struct StaticToolContext {
    pub tool_call_id: ToolCallId,
    pub session_id: SessionId,
    pub cancellation_token: CancellationToken,
    pub services: Arc<ToolServices>,
}

impl StaticToolContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum StaticToolError {
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),

    #[error("{0}")]
    Execution(ToolExecutionError),

    #[error("Missing capability: {0}")]
    MissingCapability(String),

    #[error("Cancelled")]
    Cancelled,

    #[error("Timed out")]
    Timeout,
}

impl StaticToolError {
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::InvalidParams(msg.into())
    }

    pub fn execution(error: ToolExecutionError) -> Self {
        Self::Execution(error)
    }

    pub fn missing_capability(cap: &str) -> Self {
        Self::MissingCapability(cap.to_string())
    }
}

#[async_trait]
pub trait StaticTool: Send + Sync + 'static {
    type Params: DeserializeOwned + JsonSchema + Send;
    type Output: Into<ToolResult> + Send;

    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    const REQUIRES_APPROVAL: bool;
    const REQUIRED_CAPABILITIES: Capabilities;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError>;

    fn schema() -> ToolSchema
    where
        Self: Sized,
    {
        let settings = schemars::generate::SchemaSettings::draft07().with(|s| {
            s.inline_subschemas = true;
        });
        let schema_gen = settings.into_generator();
        let input_schema = schema_gen.into_root_schema_for::<Self::Params>();

        ToolSchema {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            input_schema: input_schema.into(),
        }
    }
}

#[async_trait]
pub trait StaticToolErased: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn requires_approval(&self) -> bool;
    fn required_capabilities(&self) -> Capabilities;
    fn schema(&self) -> ToolSchema;

    async fn execute_erased(
        &self,
        params: serde_json::Value,
        ctx: &StaticToolContext,
    ) -> Result<ToolResult, StaticToolError>;
}

#[async_trait]
impl<T> StaticToolErased for T
where
    T: StaticTool,
{
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn description(&self) -> &'static str {
        T::DESCRIPTION
    }

    fn requires_approval(&self) -> bool {
        T::REQUIRES_APPROVAL
    }

    fn required_capabilities(&self) -> Capabilities {
        T::REQUIRED_CAPABILITIES
    }

    fn schema(&self) -> ToolSchema {
        T::schema()
    }

    async fn execute_erased(
        &self,
        params: serde_json::Value,
        ctx: &StaticToolContext,
    ) -> Result<ToolResult, StaticToolError> {
        let typed_params: T::Params = serde_json::from_value(params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        if ctx.is_cancelled() {
            return Err(StaticToolError::Cancelled);
        }

        let result = self.execute(typed_params, ctx).await?;
        Ok(result.into())
    }
}
