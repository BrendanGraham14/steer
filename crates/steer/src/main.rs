use clap::Parser;
use eyre::Result;

use std::io::Write;
use std::path::PathBuf;
use steer::cli::{Cli, Commands};
use steer::commands::{
    Command, headless::HeadlessCommand, serve::ServeCommand, session::SessionCommand,
    workspace::WorkspaceCommand,
};
use steer::model_resolver::resolve_model_selection;
use steer::session_config::{SessionConfigLoader, SessionConfigOverrides};
use steer::telemetry::{StartupCommand as TelemetryStartupCommand, StartupTelemetryContext};
use tracing::{debug, warn};
use uuid::Uuid;

/// Parameters for running the TUI
struct TuiParams {
    session_id: Option<String>,
    model: Option<String>,
    model_override: Option<String>,
    directory: Option<PathBuf>,
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
    model_override: Option<String>,
    directory: Option<PathBuf>,
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

    if cli.system_prompt.is_some() {
        eyre::bail!("--system-prompt is no longer supported; use primary agent policies instead");
    }

    // Load .env file if it exists
    steer::cli::config::load_env()?;

    // Initialize tracing (level configured via RUST_LOG env var)
    steer_core::utils::tracing::init_tracing()?;

    // Load preferences to get default model
    let preferences = steer_core::preferences::Preferences::load().unwrap_or_default();

    // Determine preferred model source:
    // 1. CLI argument (if provided)
    // 2. Preferences default_model (if set)
    let cli_model = cli.model.clone();
    let preference_model = preferences.default_model.clone();
    let preferred_model = cli_model.clone().or(preference_model.clone());

    let telemetry_context = StartupTelemetryContext {
        command: map_startup_command(cli.command.as_ref()),
        session_id: parse_session_uuid(cli.session.as_deref()),
        provider: provider_from_model(preferred_model.as_deref()),
        model: preferred_model.clone(),
    };
    let telemetry_preferences = preferences.telemetry.clone();
    tokio::spawn(async move {
        steer::telemetry::emit_startup_event(telemetry_context, telemetry_preferences).await;
    });

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
                // Set panic hook for terminal cleanup
                setup_panic_hook();

                // Launch TUI with appropriate backend
                if let Some(addr) = remote_addr {
                    // Merge catalogs: use subcommand if provided, otherwise fall back to global
                    let catalogs = if subcommand_catalogs.is_empty() {
                        cli.catalogs.clone()
                    } else {
                        subcommand_catalogs
                    };

                    // Connect to remote server
                    run_tui_remote(RemoteTuiParams {
                        remote_addr: addr,
                        session_id: cli.session,
                        model: preferred_model.clone(),
                        model_override: cli_model.clone(),
                        directory: cli.directory,
                        session_config_path,
                        theme: theme_name.clone(),
                        catalogs,
                        force_setup,
                    })
                    .await
                } else {
                    // Merge catalogs: use subcommand if provided, otherwise fall back to global
                    let catalogs = if subcommand_catalogs.is_empty() {
                        cli.catalogs.clone()
                    } else {
                        subcommand_catalogs
                    };

                    // Normalize and warn for invalid catalog paths
                    let catalogs = normalize_catalogs(&catalogs);

                    // Launch with in-process server
                    run_tui_local(TuiParams {
                        session_id: cli.session,
                        model: preferred_model.clone(),
                        model_override: cli_model.clone(),
                        directory: cli.directory,
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
            if system_prompt.is_some() {
                eyre::bail!(
                    "--system-prompt is no longer supported; use primary agent policies instead"
                );
            }
            let remote_addr = remote.or(cli.remote.clone());
            let catalog_paths: Vec<String> = catalogs
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let global_model = if let Some(model) = cli_model.clone() {
                model
            } else {
                let selection =
                    resolve_model_selection(preference_model.as_deref(), &catalog_paths);
                selection.default_model.to_string()
            };

            let command = HeadlessCommand {
                model: headless_model,
                messages_json,
                global_model,
                session,
                session_config,
                system_prompt,
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
            let catalogs = if server_catalogs.is_empty() {
                cli.catalogs.clone()
            } else {
                server_catalogs
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
                preferred_model: preferred_model.clone(),
            };
            command.execute().await
        }
        Commands::Workspace { workspace_command } => {
            let command = WorkspaceCommand {
                command: workspace_command,
                remote: cli.remote.clone(),
                session_id: cli.session.clone(),
            };
            command.execute().await
        }
    }
}

#[cfg(feature = "ui")]
async fn run_tui_local(params: TuiParams) -> Result<()> {
    use steer_grpc::client_api::CreateSessionParams;
    use steer_grpc::local_server;

    let mut session_id = params.session_id;

    // Set working directory if specified
    if let Some(dir) = &params.directory {
        std::env::set_current_dir(dir)?;
    }

    // Normalize and warn for invalid catalog paths
    let catalog_paths: Vec<String> = normalize_catalogs(&params.catalogs);

    let session_db_path = match params.session_db.clone() {
        Some(path) => path,
        None => steer_core::utils::session::create_session_store_path()?,
    };

    let local_grpc_setup = local_server::setup_local_grpc_with_catalog(
        steer_core::config::model::builtin::default_model(),
        Some(session_db_path),
        steer_core::catalog::CatalogConfig::with_catalogs(catalog_paths),
        None,
    )
    .await
    .map_err(|e| eyre::eyre!("Failed to setup local gRPC: {}", e))?;
    let channel = local_grpc_setup.channel;

    // Create gRPC client
    let client = steer_grpc::AgentClient::from_channel(channel)
        .await
        .map_err(|e| eyre::eyre!("Failed to create gRPC client: {}", e))?;

    let server_default = client
        .get_default_model()
        .await
        .map_err(|e| eyre::eyre!("Failed to fetch server default model: {}", e))?;

    let mut model_override = None;
    let model_id = if let Some(model_str) = params.model.as_deref() {
        match client.resolve_model(model_str).await {
            Ok(id) => {
                if params.model_override.is_some() {
                    model_override = Some(id.clone());
                }
                id
            }
            Err(e) => {
                let fallback = server_default.to_string();
                warn!(
                    "Failed to resolve preferred model '{}': {}. Using server default {}.",
                    model_str, e, fallback
                );
                let mut stderr = std::io::stderr();
                writeln!(
                    stderr,
                    "Warning: preferred model '{model_str}' is invalid. Using server default {fallback}."
                )?;
                server_default.clone()
            }
        }
    } else {
        server_default.clone()
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
                ts.as_ref().map_or((0, 0), |t| (t.seconds, t.nanos))
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
            default_model: model_override.clone(),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(model_id.clone(), params.session_config_path.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;
        let session_params = CreateSessionParams::from(session_config);

        // Create the session
        let new_session_id = client
            .create_session(session_params)
            .await
            .map_err(|e| eyre::eyre!("Failed to create session: {}", e))?;
        tracing::info!("Created session from config: {}", new_session_id);
        let mut stdout = std::io::stdout();
        writeln!(stdout, "Session ID: {new_session_id}")?;
        session_id = Some(new_session_id);
    }

    // Run TUI with the client
    tui::run_tui(
        client,
        session_id,
        model_id,
        params.directory.clone(),
        params.theme.clone(),
        params.force_setup,
    )
    .await
    .map_err(|e| eyre::eyre!("TUI error: {}", e))
}

#[cfg(feature = "ui")]
async fn run_tui_remote(params: RemoteTuiParams) -> Result<()> {
    use steer_grpc::AgentClient;
    use steer_grpc::client_api::CreateSessionParams;

    let mut session_id = params.session_id;

    // Connect to remote server
    let client = AgentClient::connect(&params.remote_addr)
        .await
        .map_err(|e| eyre::eyre!("Failed to connect to remote server: {}", e))?;

    if !params.catalogs.is_empty() {
        warn!("Ignoring --catalog for remote TUI");
    }

    // Resolve "latest" alias
    if matches!(session_id.as_deref(), Some("latest")) {
        let mut sessions = client
            .list_sessions()
            .await
            .map_err(|e| eyre::eyre!("Failed to list sessions: {}", e))?;
        sessions.sort_by(|a, b| {
            let ts_to_tuple = |ts: &Option<prost_types::Timestamp>| {
                ts.as_ref().map_or((0, 0), |t| (t.seconds, t.nanos))
            };
            ts_to_tuple(&b.updated_at).cmp(&ts_to_tuple(&a.updated_at))
        });
        if let Some(latest) = sessions.first() {
            session_id = Some(latest.id.clone());
        } else {
            eyre::bail!("No sessions found to resume");
        }
    }

    let server_default = client
        .get_default_model()
        .await
        .map_err(|e| eyre::eyre!("Failed to fetch server default model: {}", e))?;

    let mut model_override = None;
    let model_id = if let Some(model_str) = params.model.as_deref() {
        match client.resolve_model(model_str).await {
            Ok(resolved) => {
                if params.model_override.is_some() {
                    model_override = Some(resolved.clone());
                }
                resolved
            }
            Err(e) => {
                let fallback = server_default.to_string();
                warn!(
                    "Failed to resolve preferred model '{}': {}. Using server default {}.",
                    model_str, e, fallback
                );
                let mut stderr = std::io::stderr();
                writeln!(
                    stderr,
                    "Warning: preferred model '{model_str}' is invalid. Using server default {fallback}."
                )?;
                server_default.clone()
            }
        }
    } else {
        server_default.clone()
    };

    // If no session_id, we need to create a new session
    if session_id.is_none() {
        // Load session config (explicit path if provided, else auto-discovery or defaults)
        let overrides = SessionConfigOverrides {
            default_model: model_override.clone(),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(model_id.clone(), params.session_config_path.clone())
            .with_overrides(overrides);

        let session_config = loader.load().await?;
        let session_params = CreateSessionParams::from(session_config);

        // Create the session
        let new_session_id = client
            .create_session(session_params)
            .await
            .map_err(|e| eyre::eyre!("Failed to create session: {}", e))?;
        tracing::info!("Created session from config: {}", new_session_id);
        let mut stdout = std::io::stdout();
        writeln!(stdout, "Session ID: {new_session_id}")?;
        session_id = Some(new_session_id);
    }

    // Run TUI with the client
    tui::run_tui(
        client,
        session_id,
        model_id,
        params.directory.clone(),
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
            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    warn!(error = %error, "Failed to set up SIGTERM handler");
                    return;
                }
            };
            sigterm.recv().await;

            // Always clean up terminal on SIGTERM
            crate::tui::terminal::cleanup();

            std::process::exit(0);
        });

        let _sigint_task = tokio::spawn(async move {
            let mut sigint = match signal(SignalKind::interrupt()) {
                Ok(signal) => signal,
                Err(error) => {
                    warn!(error = %error, "Failed to set up SIGINT handler");
                    return;
                }
            };
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

fn map_startup_command(command: Option<&Commands>) -> TelemetryStartupCommand {
    match command {
        None | Some(Commands::Tui { .. }) => TelemetryStartupCommand::Tui,
        Some(Commands::Headless { .. }) => TelemetryStartupCommand::Headless,
        Some(Commands::Server { .. }) => TelemetryStartupCommand::Server,
        Some(
            Commands::Preferences { .. } | Commands::Session { .. } | Commands::Workspace { .. },
        ) => TelemetryStartupCommand::Unknown,
    }
}

fn parse_session_uuid(session: Option<&str>) -> Option<Uuid> {
    session.and_then(|value| Uuid::parse_str(value).ok())
}

fn provider_from_model(model: Option<&str>) -> Option<String> {
    let model = model?.trim();
    if model.is_empty() {
        return None;
    }

    if let Some((provider, _)) = model.split_once('/') {
        let provider = provider.trim().to_ascii_lowercase();
        if !provider.is_empty() {
            return Some(provider);
        }
    }

    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude") {
        Some("anthropic".to_string())
    } else if lower.starts_with("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        Some("openai".to_string())
    } else if lower.starts_with("gemini") {
        Some("gemini".to_string())
    } else if lower.starts_with("grok") {
        Some("xai".to_string())
    } else {
        None
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
#[cfg(test)]
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

    #[test]
    fn map_startup_command_defaults_to_tui() {
        assert!(matches!(
            map_startup_command(None),
            TelemetryStartupCommand::Tui
        ));
    }

    #[test]
    fn provider_from_model_extracts_custom_provider_prefix() {
        let provider = provider_from_model(Some("openai/custom-model"));
        assert_eq!(provider.as_deref(), Some("openai"));
    }

    #[test]
    fn provider_from_model_maps_builtin_aliases() {
        assert_eq!(provider_from_model(Some("opus")).as_deref(), None);
        assert_eq!(
            provider_from_model(Some("claude-sonnet-4")).as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            provider_from_model(Some("gpt-5")).as_deref(),
            Some("openai")
        );
        assert_eq!(
            provider_from_model(Some("gemini-2.5-pro")).as_deref(),
            Some("gemini")
        );
        assert_eq!(provider_from_model(Some("grok-4")).as_deref(), Some("xai"));
    }
}
