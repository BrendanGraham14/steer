pub mod cli;
pub mod commands;
pub mod error;
pub mod session_config;

// Re-export modules from conductor-core
pub use conductor_core::{api, app, config, events, runners, session, tools, utils, workspace};

use conductor_core::api::Model;
use conductor_core::app::Message;
use conductor_core::runners::{OneShotRunner, RunOnceResult};
use conductor_core::session::{SessionManager, SessionToolConfig};
use eyre::Result;

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
) -> Result<RunOnceResult> {
    OneShotRunner::run_in_session(session_manager, session_id, message)
        .await
        .map_err(|e| eyre::eyre!("Failed to run in session: {}", e))
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
    tool_policy: Option<conductor_core::session::ToolApprovalPolicy>,
    system_prompt: Option<String>,
) -> Result<RunOnceResult> {
    OneShotRunner::run_ephemeral(
        session_manager,
        init_msgs,
        model,
        tool_config,
        tool_policy,
        system_prompt,
    )
    .await
    .map_err(|e| eyre::eyre!("Failed to run ephemeral session: {}", e))
}

/// Convenience function for simple one-shot runs with default tool configuration.
/// Creates a temporary SessionManager for this single operation.
///
/// * `init_msgs`     – seed conversation (system + user or multi-turn)
/// * `model`         – which LLM to use
pub async fn run_once(init_msgs: Vec<Message>, model: Model) -> Result<RunOnceResult> {
    // Only create temporary session manager for the simple convenience function
    let session_manager = create_session_manager().await?;
    run_once_ephemeral(&session_manager, init_msgs, model, None, None, None).await
}

/// Creates a SessionManager for use with the one-shot functions.
///
/// This is the recommended way to create a SessionManager for one-shot operations
/// when you want to reuse it across multiple calls.
pub async fn create_session_manager() -> Result<SessionManager> {
    use conductor_core::session::SessionManagerConfig;

    // Use the same session store as normal operation (~/.conductor/sessions.db)
    let store = conductor_core::utils::session::create_session_store()
        .await
        .map_err(|e| eyre::eyre!("Failed to create session store: {}", e))?;

    let config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: Model::default(),
        auto_persist: true,
    };

    Ok(SessionManager::new(store, config))
}
