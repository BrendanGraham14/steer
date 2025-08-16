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
pub async fn run_once_ephemeral_with_catalog(
    session_manager: &SessionManager,
    init_msgs: Vec<Message>,
    model: String,
    tool_config: Option<SessionToolConfig>,
    tool_policy: Option<steer_core::session::ToolApprovalPolicy>,
    system_prompt: Option<String>,
    catalog_paths: Vec<String>,
) -> Result<RunOnceResult> {
    // Create AppConfig to get the model registry with custom catalogs
    let auth_storage = std::sync::Arc::new(
        steer_core::auth::DefaultAuthStorage::new()
            .map_err(|e| eyre::eyre!("Failed to create auth storage: {}", e))?,
    );
    let app_config = steer_core::app::AppConfig::from_auth_storage_with_catalog(
        auth_storage,
        steer_core::catalog::CatalogConfig::with_catalogs(catalog_paths),
    )
    .map_err(|e| eyre::eyre!("Failed to create app config: {}", e))?;

    // Resolve model string to ModelId
    let model_id = app_config
        .model_registry
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

pub async fn run_once_ephemeral(
    session_manager: &SessionManager,
    init_msgs: Vec<Message>,
    model: String,
    tool_config: Option<SessionToolConfig>,
    tool_policy: Option<steer_core::session::ToolApprovalPolicy>,
    system_prompt: Option<String>,
) -> Result<RunOnceResult> {
    run_once_ephemeral_with_catalog(
        session_manager,
        init_msgs,
        model,
        tool_config,
        tool_policy,
        system_prompt,
        Vec::new(),
    )
    .await
}

/// Creates a SessionManager for use with the one-shot functions.
///
/// This is the recommended way to create a SessionManager for one-shot operations
/// when you want to reuse it across multiple calls.
pub async fn create_session_manager(default_model: String) -> Result<SessionManager> {
    create_session_manager_with_catalog(default_model, Vec::new()).await
}

pub async fn create_session_manager_with_catalog(
    default_model: String,
    catalog_paths: Vec<String>,
) -> Result<SessionManager> {
    use steer_core::session::SessionManagerConfig;

    // Use the same session store as normal operation (~/.steer/sessions.db)
    let store = steer_core::utils::session::create_session_store()
        .await
        .map_err(|e| eyre::eyre!("Failed to create session store: {}", e))?;

    // Create AppConfig to get both registries with custom catalogs
    let auth_storage = std::sync::Arc::new(
        steer_core::auth::DefaultAuthStorage::new()
            .map_err(|e| eyre::eyre!("Failed to create auth storage: {}", e))?,
    );
    let app_config = steer_core::app::AppConfig::from_auth_storage_with_catalog(
        auth_storage,
        steer_core::catalog::CatalogConfig::with_catalogs(catalog_paths.clone()),
    )
    .map_err(|e| eyre::eyre!("Failed to create app config: {}", e))?;

    let model_id = app_config
        .model_registry
        .resolve(&default_model)
        .map_err(|e| eyre::eyre!("Invalid model: {}", e))?;

    let config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: model_id,
        auto_persist: true,
    };

    Ok(SessionManager::new(store, config))
}
