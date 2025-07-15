mod chat;
mod client;
mod responses;
mod responses_types;
mod types;

pub use client::OpenAIClient;

/// Provider name constant for OpenAI
pub(crate) const PROVIDER_NAME: &str = "openai";

#[derive(Debug, Clone, Copy)]
pub enum OpenAIMode {
    Responses,
    Chat,
}
