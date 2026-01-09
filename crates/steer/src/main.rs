use clap::Parser;
use eyre::Result;

use std::path::PathBuf;
use steer::cli::{Cli, Commands};
use steer::commands::{
    Command, headless::HeadlessCommand, serve::ServeCommand, session::SessionCommand,
};
use steer::session_config::{SessionConfigLoader, SessionConfigOverrides};
use tracing::{debug, warn};

/// Parameters for running the TUI
struct TuiParams {
    session_id: Option<String>,
    model: Option<String>,
    directory: Option<PathBuf>,
    system_prompt: Option<String>,
    session_db: Option<PathBuf>,
    session_config_path: Option<PathBuf>,
    theme: Option<String>,
    catalogs: Vec<PathBuf>,
    force_setup: bool,
}

/// Parameters for running the TUI with a remote server
struct RemoteTuiParams {
    remote_addr: String,
    session_id: Option<String>,
    model: Option<String>,
    directory: Option<PathBuf>,
    system_prompt: Option<String>,
    session_config_path: Option<PathBuf>,
    theme: Option<String>,
    catalogs: Vec<PathBuf>,
    force_setup: bool,
}

#[cfg(feature = "ui")]
use steer_tui::tui::{self, setup_panic_hook};

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre for better error reports
    color_eyre::install()?;

    let cli = Cli::parse();

    // Load .env file if it exists
    steer::cli::config::load_env()?;

    // Initialize tracing (level configured via RUST_LOG env var)
    steer_core::utils::tracing::init_tracing()?;

    // Load preferences to get default model
    let preferences = steer_core::preferences::Preferences::load().unwrap_or_default();

    // Determine which model to use:
    // 1. CLI argument (if provided)
    // 2. Preferences default_model (if set)
    // 3. System default
    let preferred_model = cli.model.clone().or(preferences.default_model.clone());
    let chosen_model = preferred_model
        .clone()
        .unwrap_or_else(|| "opus".to_string());

    // Set up signal handlers for terminal cleanup if using TUI
    #[cfg(feature = "ui")]
    if cli.command.is_none() || matches!(cli.command, Some(Commands::Tui { .. })) {
        setup_signal_handlers().await;
    }

    // If no subcommand specified, default to TUI
    let cmd = cli.command.clone().unwrap_or(Commands::Tui {
        remote: None,         // Will use global --remote if set
        session_config: None, // Will use global --session-config if set
        theme: None,          // Will use global --theme if set
        catalogs: vec![],     // Will use global --catalog if set
        force_setup: cli.force_setup,
    });

    match cmd {
        Commands::Tui {
            remote,
            session_config,
            theme,
            catalogs: subcommand_catalogs,
            force_setup,
        } => {
            #[cfg(feature = "ui")]
            {
                // Use subcommand remote if provided, otherwise fall back to global
                let remote_addr = remote.or(cli.remote.clone());
                // Use subcommand session_config if provided, otherwise fall back to global
                let session_config_path = session_config.or(cli.session_config.clone());
                // Use subcommand theme if provided, otherwise fall back to global
                let theme_name = theme.or(cli.theme.clone());
                // Merge catalogs: use subcommand if provided, otherwise fall back to global
                let catalogs = if !subcommand_catalogs.is_empty() {
                    subcommand_catalogs
                } else {
                    cli.catalogs.clone()
                };

                // Normalize and warn for invalid catalog paths
                let catalogs = normalize_catalogs(&catalogs);

                // Set panic hook for terminal cleanup
                setup_panic_hook();

                // Launch TUI with appropriate backend
                if let Some(addr) = remote_addr {
                    // Connect to remote server
                    run_tui_remote(RemoteTuiParams {
                        remote_addr: addr,
                        session_id: cli.session,
                        model: preferred_model.clone(),
                        directory: cli.directory,
                        system_prompt: cli.system_prompt,
                        session_config_path,
                        theme: theme_name.clone(),
                        catalogs: catalogs.iter().map(PathBuf::from).collect(),
                        force_setup,
                    })
                    .await
                } else {
                    // Launch with in-process server
                    run_tui_local(TuiParams {
                        session_id: cli.session,
                        model: preferred_model.clone(),
                        directory: cli.directory,
                        system_prompt: cli.system_prompt,
                        session_db: cli.session_db,
                        session_config_path,
                        theme: theme_name,
                        catalogs: catalogs.iter().map(PathBuf::from).collect(),
                        force_setup,
                    })
                    .await
                }
            }
            #[cfg(not(feature = "ui"))]
            {
                eyre::bail!(
                    "Terminal UI not available. This binary was compiled without the 'ui' feature."
                );
            }
        }
        Commands::Preferences { action } => {
            use steer::cli::args::PreferencesCommands;
            use steer::commands::preferences::{PreferencesAction, PreferencesCommand};
            let cmd = PreferencesCommand {
                action: match action {
                    PreferencesCommands::Show => PreferencesAction::Show,
                    PreferencesCommands::Edit => PreferencesAction::Edit,
                    PreferencesCommands::Reset => PreferencesAction::Reset,
                },
            };
            cmd.execute().await
        }
        Commands::Headless {
            model: headless_model,
            messages_json,
            session,
            session_config,
            system_prompt,
            remote,
            catalogs,
        } => {
            // Parse headless model if provided, otherwise use global model
            let effective_model = headless_model.as_ref().unwrap_or(&chosen_model);
            let remote_addr = remote.or(cli.remote.clone());

            let command = HeadlessCommand {
                model: Some(effective_model.clone()),
                messages_json,
                global_model: effective_model.clone(),
                session,
                session_config,
                system_prompt: system_prompt.or(cli.system_prompt),
                remote: remote_addr,
                directory: cli.directory,
                catalogs,
            };
            command.execute().await
        }
        Commands::Server {
            port,
            bind,
            catalogs: server_catalogs,
        } => {
            // Merge catalogs: prefer subcommand if provided, else use global
            let catalogs = if !server_catalogs.is_empty() {
                server_catalogs
            } else {
                cli.catalogs.clone()
            };
            let catalogs = normalize_catalogs(&catalogs);

            let command = ServeCommand {
                port,
                bind,
                session_db: cli.session_db.clone(),
                catalogs: catalogs.iter().map(PathBuf::from).collect(),
            };
            command.execute().await
        }
        Commands::Session { session_command } => {
            let command = SessionCommand {
                command: session_command,
                remote: cli.remote.clone(),
                session_db: cli.session_db.clone(),
                catalogs: cli.catalogs.clone(),
            };
            command.execute().await
        }
        Commands::Notify {
            title,
            message,
            sound,
        } => {
            let sound_type = sound.and_then(|s| {
                s.parse::<steer_tui::notifications::NotificationSound>()
                    .ok()
            });
            steer_tui::notifications::show_notification_with_sound(&title, &message, sound_type)
                .map_err(|e| eyre::eyre!("Failed to show notification: {}", e))
        }
    }
}

#[cfg(feature = "ui")]
async fn run_tui_local(params: TuiParams) -> Result<()> {
    use steer_grpc::local_server;

    let mut session_id = params.session_id;

    // Set working directory if specified
    if let Some(dir) = &params.directory {
        std::env::set_current_dir(dir)?;
    }

    // Normalize and warn for invalid catalog paths
    let catalog_paths: Vec<String> = normalize_catalogs(&params.catalogs);

    // Prefer the requested model (CLI or preferences) as the server default so we
    // don't immediately try to use the Anthropic builtin when the user wants
    // something else.
    let preferred_default_model = match (
        &params.model,
        steer_core::model_registry::ModelRegistry::load(&catalog_paths),
    ) {
        (Some(model_str), Ok(registry)) => match registry.resolve(model_str) {
            Ok(id) => Some(id),
            Err(e) => {
                warn!(
                    "Failed to resolve preferred model '{}': {}. Falling back to builtin default.",
                    model_str, e
                );
                None
            }
        },
        (None, Ok(_)) => None,
        (_, Err(e)) => {
            warn!(
                "Failed to load model registry for catalogs {:?}: {}. Falling back to builtin default.",
                catalog_paths, e
            );
            None
        }
    };

    // Use the preferred model if we resolved it, otherwise fall back to builtin default
    let default_model = preferred_default_model
        .clone()
        .unwrap_or_else(steer_core::config::model::builtin::opus);

    let local_grpc_setup = local_server::setup_local_grpc_with_catalog(
        default_model.clone(),
        params.session_db.clone(),
        steer_core::catalog::CatalogConfig::with_catalogs(catalog_paths),
    )
    .await
    .map_err(|e| eyre::eyre!("Failed to setup local gRPC: {}", e))?;
    let channel = local_grpc_setup.channel;

    // Create gRPC client
    let client = steer_grpc::AgentClient::from_channel(channel)
        .await
        .map_err(|e| eyre::eyre!("Failed to create gRPC client: {}", e))?;

    let model_id = if let Some(model) = preferred_default_model {
        model
    } else if let Some(model_str) = params.model.as_deref() {
        match client.resolve_model(model_str).await {
            Ok(id) => id,
            Err(e) => {
                warn!(
                    "Failed to resolve model '{}': {}. Falling back to default.",
                    model_str, e
                );
                default_model
            }
        }
    } else {
        default_model
    };

    // Resolve "latest" alias
    if matches!(session_id.as_deref(), Some("latest")) {
        let mut sessions = client
            .list_sessions()
            .await
            .map_err(|e| eyre::eyre!("Failed to list sessions: {}", e))?;
        // Sort sessions by updated_at descending, fallback to created_at
        sessions.sort_by(|a, b| {
            let ts_to_tuple = |ts: &Option<prost_types::Timestamp>| {
                ts.as_ref().map(|t| (t.seconds, t.nanos)).unwrap_or((0, 0))
            };
            ts_to_tuple(&b.updated_at).cmp(&ts_to_tuple(&a.updated_at))
        });
        if let Some(latest) = sessions.first() {
            session_id = Some(latest.id.clone());
        } else {
            eyre::bail!("No sessions found to resume");
        }
    }

    // If no session_id, we need to create a new session
    if session_id.is_none() {
        // Load session config (explicit path if provided, else auto-discovery or defaults)
        let overrides = SessionConfigOverrides {
            system_prompt: params.system_prompt.clone(),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(model_id.clone(), params.session_config_path.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;

        // Create the session
        let new_session_id = client
            .create_session(session_config)
            .await
            .map_err(|e| eyre::eyre!("Failed to create session: {}", e))?;
        tracing::info!("Created session from config: {}", new_session_id);
        println!("Session ID: {new_session_id}");
        session_id = Some(new_session_id);
    }

    // Run TUI with the client
    tui::run_tui(
        client,
        session_id,
        model_id,
        params.directory.clone(),
        None, // system_prompt is already applied to the session
        params.theme.clone(),
        params.force_setup,
    )
    .await
    .map_err(|e| eyre::eyre!("TUI error: {}", e))
}

#[cfg(feature = "ui")]
async fn run_tui_remote(params: RemoteTuiParams) -> Result<()> {
    use steer_grpc::AgentClient;

    let mut session_id = params.session_id;

    // Connect to remote server
    let client = AgentClient::connect(&params.remote_addr)
        .await
        .map_err(|e| eyre::eyre!("Failed to connect to remote server: {}", e))?;

    // Resolve "latest" alias
    if matches!(session_id.as_deref(), Some("latest")) {
        let mut sessions = client
            .list_sessions()
            .await
            .map_err(|e| eyre::eyre!("Failed to list sessions: {}", e))?;
        sessions.sort_by(|a, b| {
            let ts_to_tuple = |ts: &Option<prost_types::Timestamp>| {
                ts.as_ref().map(|t| (t.seconds, t.nanos)).unwrap_or((0, 0))
            };
            ts_to_tuple(&b.updated_at).cmp(&ts_to_tuple(&a.updated_at))
        });
        if let Some(latest) = sessions.first() {
            session_id = Some(latest.id.clone());
        } else {
            eyre::bail!("No sessions found to resume");
        }
    }

    // Use builtin default model for TUI initially
    let default_model = steer_core::config::model::builtin::opus();

    // Try resolving via local catalogs first (if provided), then fall back to server
    let model_id = if let Some(model_str) = params.model.as_deref() {
        // Attempt local resolution using provided catalogs
        let catalog_paths: Vec<String> = normalize_catalogs(&params.catalogs);
        let locally_resolved: Option<steer_core::config::model::ModelId> =
            resolve_model_locally(model_str, &catalog_paths);

        if let Some(id) = locally_resolved {
            id
        } else {
            match client.resolve_model(model_str).await {
                Ok(resolved) => resolved,
                Err(e) => {
                    tracing::warn!(
                        "Failed to resolve model '{}' via server: {}, using default",
                        model_str,
                        e
                    );
                    default_model
                }
            }
        }
    } else {
        default_model
    };

    // If no session_id, we need to create a new session
    if session_id.is_none() {
        // Load session config (explicit path if provided, else auto-discovery or defaults)
        let overrides = SessionConfigOverrides {
            system_prompt: params.system_prompt.clone(),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(model_id.clone(), params.session_config_path.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;

        // Create the session
        let new_session_id = client
            .create_session(session_config)
            .await
            .map_err(|e| eyre::eyre!("Failed to create session: {}", e))?;
        tracing::info!("Created session from config: {}", new_session_id);
        println!("Session ID: {new_session_id}");
        session_id = Some(new_session_id);
    }

    // Run TUI with the client
    tui::run_tui(
        client,
        session_id,
        model_id,
        params.directory.clone(),
        None, // system_prompt is already applied to the session
        params.theme.clone(),
        params.force_setup,
    )
    .await
    .map_err(|e| eyre::eyre!("TUI error: {}", e))
}

#[cfg(feature = "ui")]
async fn setup_signal_handlers() {
    // Set up signal handler for SIGINT, SIGTERM
    #[cfg(not(windows))]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let _sigterm_task = tokio::spawn(async move {
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
            sigterm.recv().await;

            // Always clean up terminal on SIGTERM
            crate::tui::terminal::cleanup();

            std::process::exit(0);
        });

        let _sigint_task = tokio::spawn(async move {
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");
            sigint.recv().await;

            // Always clean up terminal on SIGINT
            crate::tui::terminal::cleanup();
            std::process::exit(130); // Standard exit code for SIGINT
        });
    }

    #[cfg(windows)]
    {
        let _ctrl_c_task = tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            // Always clean up terminal on Ctrl+C
            crate::tui::terminal::cleanup();
            std::process::exit(130);
        });
    }
}

/// Normalize catalog paths: canonicalize when possible and warn on missing or invalid paths
fn normalize_catalogs(paths: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        if !p.exists() {
            warn!("Catalog path does not exist: {}", p.display());
            out.push(p.to_string_lossy().to_string());
            continue;
        }
        match p.canonicalize() {
            Ok(c) => out.push(c.to_string_lossy().to_string()),
            Err(e) => {
                warn!("Failed to canonicalize catalog path {}: {}", p.display(), e);
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    debug!("Using catalog paths: {:?}", out);
    out
}

/// Try to resolve a model locally using provided catalog paths. Falls back to discovered catalogs.
fn resolve_model_locally(
    model: &str,
    catalog_paths: &[String],
) -> Option<steer_core::config::model::ModelId> {
    // Load a standalone registry using only explicit catalogs (discovered catalogs are already handled inside load())
    let registry = match steer_core::model_registry::ModelRegistry::load(catalog_paths) {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to load local catalogs for model resolution: {}", e);
            return None;
        }
    };

    match registry.resolve(model) {
        Ok(id) => Some(id),
        Err(e) => {
            debug!("Local resolution failed for '{}': {}", model, e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_normalize_catalogs_warns_and_keeps_paths() {
        let tmp = TempDir::new().unwrap();
        let existing = tmp.path().join("exists.toml");
        fs::write(&existing, "").unwrap();
        let missing = tmp.path().join("missing.toml");
        let out = normalize_catalogs(&[existing.clone(), missing.clone()]);
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("exists.toml"));
        assert!(out[1].contains("missing.toml"));
    }

    #[test]
    fn test_resolve_model_locally_works_with_discovery() {
        let res = resolve_model_locally("opus", &[]);
        // Should resolve using embedded + discovered catalogs
        assert!(res.is_some());
    }
}
