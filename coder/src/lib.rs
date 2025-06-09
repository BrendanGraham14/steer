pub mod api;
pub mod app;
pub mod cli;
pub mod commands;
pub mod config;
pub mod events;
pub mod grpc;
pub mod runners;
pub mod session;
pub mod tools;
pub mod tui;
pub mod utils;

use api::Model;
use app::Message;
use runners::{OneShotRunner, RunOnceResult};
use session::{SessionManager, SessionToolConfig};

/// Runs the agent once in an existing session.
///
/// * `session_manager` – SessionManager instance to use
/// * `session_id`      – ID of the existing session to use
/// * `message`         – user message to process
/// * `timeout`         – optional wall-clock limit
pub async fn run_once_in_session(
    session_manager: &SessionManager,
    session_id: String,
    message: String,
) -> anyhow::Result<RunOnceResult> {
    OneShotRunner::run_in_session(session_manager, session_id, message).await
}

/// Runs the agent once in a new ephemeral session.
///
/// * `session_manager` – SessionManager instance to use
/// * `init_msgs`       – seed conversation (system + user or multi-turn)
/// * `model`           – which LLM to use
/// * `tool_config`     – optional tool configuration
/// * `tool_policy`     – optional tool approval policy
/// * `timeout`         – optional wall-clock limit
pub async fn run_once_ephemeral(
    session_manager: &SessionManager,
    init_msgs: Vec<Message>,
    model: Model,
    tool_config: Option<SessionToolConfig>,
    tool_policy: Option<session::ToolApprovalPolicy>,
) -> anyhow::Result<RunOnceResult> {
    OneShotRunner::run_ephemeral(session_manager, init_msgs, model, tool_config, tool_policy).await
}

/// Convenience function for simple one-shot runs with default tool configuration.
/// Creates a temporary SessionManager for this single operation.
///
/// * `init_msgs`     – seed conversation (system + user or multi-turn)
/// * `model`         – which LLM to use
pub async fn run_once(init_msgs: Vec<Message>, model: Model) -> anyhow::Result<RunOnceResult> {
    // Only create temporary session manager for the simple convenience function
    let session_manager = create_session_manager().await?;
    run_once_ephemeral(&session_manager, init_msgs, model, None, None).await
}

/// Creates a SessionManager for use with the one-shot functions.
///
/// This is the recommended way to create a SessionManager for one-shot operations
/// when you want to reuse it across multiple calls.
pub async fn create_session_manager() -> anyhow::Result<SessionManager> {
    use session::SessionManagerConfig;
    use tokio::sync::mpsc;

    // Use the same session store as normal operation (~/.coder/sessions.db)
    let store = crate::utils::session::create_session_store().await?;

    let (event_tx, _event_rx) = mpsc::channel(100);
    let config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: Model::ClaudeSonnet4_20250514,
        auto_persist: true,
    };

    Ok(SessionManager::new(store, config, event_tx))
}
