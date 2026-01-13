use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ToolSpec;
use crate::error::ToolExecutionError;
use crate::result::FetchResult;

pub const FETCH_TOOL_NAME: &str = "web_fetch";

pub struct FetchToolSpec;

impl ToolSpec for FetchToolSpec {
    type Params = FetchParams;
    type Result = FetchResult;
    type Error = FetchError;

    const NAME: &'static str = FETCH_TOOL_NAME;
    const DISPLAY_NAME: &'static str = "Fetch URL";

    fn execution_error(error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::Fetch(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum FetchError {
    #[error("request failed: {message}")]
    RequestFailed { message: String },

    #[error("http error: {status} when fetching {url}")]
    Http { status: u16, url: String },

    #[error("failed to read response body from {url}: {message}")]
    ReadFailed { url: String, message: String },

    #[error("model call failed: {message}")]
    ModelCallFailed { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchParams {
    /// The URL to fetch content from
    pub url: String,
    /// The prompt to process the content with
    pub prompt: String,
}
