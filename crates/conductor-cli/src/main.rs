use clap::Parser;
use eyre::Result;

use conductor_cli::cli::{Cli, Commands};
use conductor_cli::commands::{
    Command, headless::HeadlessCommand, init::InitCommand, serve::ServeCommand,
    session::SessionCommand,
};
use conductor_cli::session_config::{SessionConfigLoader, SessionConfigOverrides};

#[cfg(feature = "ui")]
use conductor_tui::tui::{self, cleanup_terminal, setup_panic_hook};

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre for better error reports
    color_eyre::install()?;

    let cli = Cli::parse();

    // Load .env file if it exists
    conductor_cli::cli::config::load_env()?;

    // Initialize tracing (level configured via RUST_LOG env var)
    conductor_core::utils::tracing::init_tracing()?;

    // Convert ModelArg to Model
    let model: conductor_core::api::Model = cli.model.into();

    // Set up signal handlers for terminal cleanup if using TUI
    #[cfg(feature = "ui")]
    if cli.command.is_none() || matches!(cli.command, Some(Commands::Tui { .. })) {
        setup_signal_handlers().await;
    }

    // If no subcommand specified, default to TUI
    let cmd = cli.command.clone().unwrap_or(Commands::Tui {
        remote: None,         // Will use global --remote if set
        session_config: None, // Will use global --session-config if set
    });

    match cmd {
        Commands::Tui {
            remote,
            session_config,
        } => {
            #[cfg(feature = "ui")]
            {
                // Use subcommand remote if provided, otherwise fall back to global
                let remote_addr = remote.or(cli.remote.clone());
                // Use subcommand session_config if provided, otherwise fall back to global
                let session_config_path = session_config.or(cli.session_config.clone());

                // Set panic hook for terminal cleanup
                setup_panic_hook();

                // Launch TUI with appropriate backend
                if let Some(addr) = remote_addr {
                    // Connect to remote server
                    run_tui_remote(
                        addr,
                        cli.session,
                        model,
                        cli.directory,
                        cli.system_prompt,
                        session_config_path,
                    )
                    .await
                } else {
                    // Launch with in-process server
                    run_tui_local(
                        cli.session,
                        model,
                        cli.directory,
                        cli.system_prompt,
                        cli.session_db,
                        session_config_path,
                    )
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
        Commands::Init { force } => {
            let command = InitCommand { force };
            command.execute().await
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
    }
}

#[cfg(feature = "ui")]
async fn run_tui_local(
    mut session_id: Option<String>,
    model: conductor_core::api::Model,
    directory: Option<std::path::PathBuf>,
    system_prompt: Option<String>,
    session_db: Option<std::path::PathBuf>,
    session_config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    use conductor_grpc::local_server;
    use std::sync::Arc;

    // Set working directory if specified
    if let Some(dir) = &directory {
        std::env::set_current_dir(dir)?;
    }

    // Create LLM config
    let llm_config = conductor_core::config::LlmConfig::from_env()
        .expect("Failed to load LLM configuration from environment variables.");

    // Create in-memory channel
    let channel = local_server::setup_local_grpc(llm_config, model, session_db)
        .await
        .map_err(|e| eyre::eyre!("Failed to setup local gRPC: {}", e))?;

    // Create gRPC client
    let client = conductor_grpc::GrpcClientAdapter::from_channel(channel)
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
    if session_id.is_none() && (session_config_path.is_some() || system_prompt.is_some()) {
        // Load session config from file if provided
        let overrides = SessionConfigOverrides {
            system_prompt: system_prompt.clone(),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(session_config_path).with_overrides(overrides);

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
        Arc::new(client),
        session_id,
        model,
        directory,
        None, // system_prompt is already applied to the session
    )
    .await
    .map_err(|e| eyre::eyre!("TUI error: {}", e))
}

#[cfg(feature = "ui")]
async fn run_tui_remote(
    remote_addr: String,
    mut session_id: Option<String>,
    model: conductor_core::api::Model,
    directory: Option<std::path::PathBuf>,
    system_prompt: Option<String>,
    session_config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    use conductor_grpc::GrpcClientAdapter;
    use std::sync::Arc;

    // Connect to remote server
    let client = GrpcClientAdapter::connect(&remote_addr)
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
    if session_id.is_none() && (session_config_path.is_some() || system_prompt.is_some()) {
        // Load session config from file if provided
        let overrides = SessionConfigOverrides {
            system_prompt: system_prompt.clone(),
            ..Default::default()
        };

        let loader = SessionConfigLoader::new(session_config_path).with_overrides(overrides);

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
        Arc::new(client),
        session_id,
        model,
        directory,
        None, // system_prompt is already applied to the session
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
