mod chat;
mod client;
mod codex;
mod responses;
mod responses_types;
mod types;

pub use client::OpenAIClient;
pub use codex::CodexClient;

/// Provider name constant for OpenAI
pub(crate) const PROVIDER_NAME: &str = "openai";
/// Default HTTP timeout for OpenAI requests (30 minutes)
pub(crate) const HTTP_TIMEOUT_SECS: u64 = 1800;

#[derive(Debug, Clone, Copy)]
pub enum OpenAIMode {
    Responses,
    Chat,
}
