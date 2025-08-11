pub mod cli;
pub mod commands;
pub mod error;
pub mod session_config;

pub use steer_core::{api, app, config, events, runners, session, tools, utils, workspace};

use eyre::Result;
use steer_core::app::Message;
use steer_core::runners::{OneShotRunner, RunOnceResult};
use steer_core::session::{SessionManager, SessionToolConfig};

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
    model: String,
    tool_config: Option<SessionToolConfig>,
    tool_policy: Option<steer_core::session::ToolApprovalPolicy>,
    system_prompt: Option<String>,
) -> Result<RunOnceResult> {
    // Load model registry to resolve the model string
    let model_registry = steer_core::model_registry::ModelRegistry::load()
        .map_err(|e| eyre::eyre!("Failed to load model registry: {}", e))?;

    // Resolve model string to ModelId
    let model_id = model_registry
        .resolve(&model)
        .map_err(|e| eyre::eyre!("Invalid model: {}", e))?;

    OneShotRunner::run_ephemeral(
        session_manager,
        init_msgs,
        model_id,
        tool_config,
        tool_policy,
        system_prompt,
    )
    .await
    .map_err(|e| eyre::eyre!("Failed to run ephemeral session: {}", e))
}

/// Creates a SessionManager for use with the one-shot functions.
///
/// This is the recommended way to create a SessionManager for one-shot operations
/// when you want to reuse it across multiple calls.
pub async fn create_session_manager(default_model: String) -> Result<SessionManager> {
    use steer_core::session::SessionManagerConfig;

    // Use the same session store as normal operation (~/.steer/sessions.db)
    let store = steer_core::utils::session::create_session_store()
        .await
        .map_err(|e| eyre::eyre!("Failed to create session store: {}", e))?;

    // Load model registry and resolve default model
    let model_registry = steer_core::model_registry::ModelRegistry::load()
        .map_err(|e| eyre::eyre!("Failed to load model registry: {}", e))?;

    let model_id = model_registry
        .resolve(&default_model)
        .map_err(|e| eyre::eyre!("Invalid model: {}", e))?;

    let config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: model_id,
        auto_persist: true,
    };

    Ok(SessionManager::new(store, config))
}
