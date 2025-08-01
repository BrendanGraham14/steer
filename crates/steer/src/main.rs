use clap::Parser;
use eyre::Result;

use std::path::PathBuf;
use steer::cli::{Cli, Commands};
use steer::commands::{
    Command, headless::HeadlessCommand, serve::ServeCommand, session::SessionCommand,
};
use steer::session_config::{SessionConfigLoader, SessionConfigOverrides};
use steer_core::api::Model;

/// Parameters for running the TUI
struct TuiParams {
    session_id: Option<String>,
    model: Model,
    directory: Option<PathBuf>,
    system_prompt: Option<String>,
    session_db: Option<PathBuf>,
    session_config_path: Option<PathBuf>,
    theme: Option<String>,
    force_setup: bool,
}

/// Parameters for running the TUI with a remote server
struct RemoteTuiParams {
    remote_addr: String,
    session_id: Option<String>,
    model: Model,
    directory: Option<PathBuf>,
    system_prompt: Option<String>,
    session_config_path: Option<PathBuf>,
    theme: Option<String>,
    force_setup: bool,
}

#[cfg(feature = "ui")]
use steer_tui::tui::{self, cleanup_terminal, setup_panic_hook};

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
    let model: steer_core::api::Model = if let Some(model_arg) = cli.model {
        model_arg.into()
    } else if let Some(ref default_model_str) = preferences.default_model {
        // Try to parse the model from preferences
        default_model_str.parse().unwrap_or_else(|_| {
            eprintln!("Warning: Invalid model '{default_model_str}' in preferences, using default");
            Model::default()
        })
    } else {
        Model::default()
    };

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
        force_setup: cli.force_setup,
    });

    match cmd {
        Commands::Tui {
            remote,
            session_config,
            theme,
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
                    // Connect to remote server
                    run_tui_remote(RemoteTuiParams {
                        remote_addr: addr,
                        session_id: cli.session,
                        model,
                        directory: cli.directory,
                        system_prompt: cli.system_prompt,
                        session_config_path,
                        theme: theme_name.clone(),
                        force_setup,
                    })
                    .await
                } else {
                    // Launch with in-process server
                    run_tui_local(TuiParams {
                        session_id: cli.session,
                        model,
                        directory: cli.directory,
                        system_prompt: cli.system_prompt,
                        session_db: cli.session_db,
                        session_config_path,
                        theme: theme_name,
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
        } => {
            // Use headless model if provided, otherwise global model
            let effective_model = headless_model.map(Into::into).unwrap_or(model);
            let remote_addr = remote.or(cli.remote.clone());

            let command = HeadlessCommand {
                model: Some(effective_model),
                messages_json,
                global_model: effective_model,
                session,
                session_config,
                system_prompt: system_prompt.or(cli.system_prompt),
                remote: remote_addr,
                directory: cli.directory,
            };
            command.execute().await
        }
        Commands::Server { port, bind } => {
            let command = ServeCommand {
                port,
                bind,
                model,
                session_db: cli.session_db.clone(),
            };
            command.execute().await
        }
        Commands::Session { session_command } => {
            let command = SessionCommand {
                command: session_command,
                remote: cli.remote.clone(),
                session_db: cli.session_db.clone(),
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

    // Create in-memory channel
    let (channel, _server_handle) =
        local_server::setup_local_grpc(params.model, params.session_db.clone())
            .await
            .map_err(|e| eyre::eyre!("Failed to setup local gRPC: {}", e))?;

    // Create gRPC client
    let client = steer_grpc::AgentClient::from_channel(channel)
        .await
        .map_err(|e| eyre::eyre!("Failed to create gRPC client: {}", e))?;

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
    if session_id.is_none()
        && (params.session_config_path.is_some() || params.system_prompt.is_some())
    {
        // Load session config from file if provided
        let overrides = SessionConfigOverrides {
            system_prompt: params.system_prompt.clone(),
            ..Default::default()
        };

        let loader =
            SessionConfigLoader::new(params.session_config_path.clone()).with_overrides(overrides);

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
        params.model,
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

    // If no session_id, we need to create a new session
    if session_id.is_none()
        && (params.session_config_path.is_some() || params.system_prompt.is_some())
    {
        // Load session config from file if provided
        let overrides = SessionConfigOverrides {
            system_prompt: params.system_prompt.clone(),
            ..Default::default()
        };

        let loader =
            SessionConfigLoader::new(params.session_config_path.clone()).with_overrides(overrides);

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
        params.model,
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Set up a flag to track terminal state for signal handlers
    let terminal_in_raw_mode = Arc::new(AtomicBool::new(false));
    let terminal_clone = terminal_in_raw_mode.clone();

    // Set up signal handler for SIGINT, SIGTERM
    #[cfg(not(windows))]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let sigterm_terminal = terminal_clone.clone();
        let _sigterm_task = tokio::spawn(async move {
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
            sigterm.recv().await;

            // Clean up terminal if in raw mode
            if sigterm_terminal.load(Ordering::Relaxed) {
                cleanup_terminal();
            }
            std::process::exit(0);
        });

        let sigint_terminal = terminal_clone.clone();
        let _sigint_task = tokio::spawn(async move {
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");
            sigint.recv().await;

            // Clean up terminal if in raw mode
            if sigint_terminal.load(Ordering::Relaxed) {
                cleanup_terminal();
            }
            std::process::exit(130); // Standard exit code for SIGINT
        });
    }

    #[cfg(windows)]
    {
        let windows_terminal = terminal_clone;
        let _ctrl_c_task = tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            if windows_terminal.load(Ordering::Relaxed) {
                cleanup_terminal();
            }
            std::process::exit(130);
        });
    }
}
