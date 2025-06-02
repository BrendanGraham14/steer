pub mod api;
pub mod app;
pub mod config;
pub mod events;
pub mod grpc;
pub mod runners;
pub mod session;
pub mod tools;
pub mod tui;
pub mod utils;

use api::{Model, messages::Message};
use config::LlmConfig;
use runners::{OneShotRunner, RunOnceResult};
use std::time::Duration;

/// Runs the agent once and waits for the final assistant message.
///
/// * `init_msgs` – seed conversation (system + user or multi-turn)
/// * `model`     – which LLM to use
/// * `cfg`       – LLM config with API keys
/// * `timeout`   – optional wall-clock limit
pub async fn run_once(
    init_msgs: Vec<Message>,
    model: Model,
    cfg: &LlmConfig,
    timeout: Option<Duration>,
) -> anyhow::Result<RunOnceResult> {
    let system_prompt = Some(include_str!("../prompts/system_prompt.md").to_string());
    let runner = OneShotRunner::new(cfg);
    runner.run(init_msgs, model, system_prompt, timeout).await
}
