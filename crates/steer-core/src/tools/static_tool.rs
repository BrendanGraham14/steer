use std::error::Error as StdError;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use crate::app::domain::types::{SessionId, ToolCallId};
use crate::config::model::ModelId;
use steer_tools::error::ToolExecutionError;
use steer_tools::result::ToolResult;
use steer_tools::{ToolSchema, ToolSpec};

use super::capability::Capabilities;
use super::services::ToolServices;

#[derive(Debug, Clone)]
pub struct StaticToolContext {
    pub tool_call_id: ToolCallId,
    pub session_id: SessionId,
    pub invoking_model: Option<ModelId>,
    pub cancellation_token: CancellationToken,
    pub services: Arc<ToolServices>,
}

impl StaticToolContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StaticToolError<E: StdError + Send + Sync + 'static> {
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),

    #[error("{0}")]
    Execution(E),

    #[error("Missing capability: {0}")]
    MissingCapability(String),

    #[error("Cancelled")]
    Cancelled,

    #[error("Timed out")]
    Timeout,
}

impl<E: StdError + Send + Sync + 'static> StaticToolError<E> {
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::InvalidParams(msg.into())
    }

    pub fn execution(error: E) -> Self {
        Self::Execution(error)
    }

    pub fn missing_capability(cap: &str) -> Self {
        Self::MissingCapability(cap.to_string())
    }

    pub fn map_execution<F, E2>(self, f: F) -> StaticToolError<E2>
    where
        F: FnOnce(E) -> E2,
        E2: StdError + Send + Sync + 'static,
    {
        match self {
            StaticToolError::InvalidParams(msg) => StaticToolError::InvalidParams(msg),
            StaticToolError::Execution(err) => StaticToolError::Execution(f(err)),
            StaticToolError::MissingCapability(cap) => StaticToolError::MissingCapability(cap),
            StaticToolError::Cancelled => StaticToolError::Cancelled,
            StaticToolError::Timeout => StaticToolError::Timeout,
        }
    }
}

#[async_trait]
pub trait StaticTool: Send + Sync + 'static {
    type Params: DeserializeOwned + JsonSchema + Send;
    type Output: Into<ToolResult> + Send;
    type Spec: ToolSpec<Params = Self::Params, Result = Self::Output>;

    const DESCRIPTION: &'static str;
    const REQUIRES_APPROVAL: bool;
    const REQUIRED_CAPABILITIES: Capabilities;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError<<Self::Spec as ToolSpec>::Error>>;

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
            name: Self::Spec::NAME.to_string(),
            display_name: Self::Spec::DISPLAY_NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            input_schema: input_schema.into(),
        }
    }
}

#[async_trait]
pub trait StaticToolErased: Send + Sync {
    fn name(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn requires_approval(&self) -> bool;
    fn required_capabilities(&self) -> Capabilities;
    fn schema(&self) -> ToolSchema;

    async fn execute_erased(
        &self,
        params: serde_json::Value,
        ctx: &StaticToolContext,
    ) -> Result<ToolResult, StaticToolError<ToolExecutionError>>;
}

#[async_trait]
impl<T> StaticToolErased for T
where
    T: StaticTool,
{
    fn name(&self) -> &'static str {
        T::Spec::NAME
    }

    fn display_name(&self) -> &'static str {
        T::Spec::DISPLAY_NAME
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
    ) -> Result<ToolResult, StaticToolError<ToolExecutionError>> {
        let typed_params: T::Params = serde_json::from_value(params)
            .map_err(|e| StaticToolError::invalid_params(e.to_string()))?;

        if ctx.is_cancelled() {
            return Err(StaticToolError::Cancelled);
        }

        let result = self
            .execute(typed_params, ctx)
            .await
            .map_err(|e| e.map_execution(T::Spec::execution_error))?;
        Ok(result.into())
    }
}
