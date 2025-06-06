use anyhow::{Result, anyhow};
use clap::Parser;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tracing::info;

use coder::api::Model;
use coder::app::AppConfig;
use coder::cli::{Cli, Commands};
use coder::commands::{
    Command, headless::HeadlessCommand, init::InitCommand, serve::ServeCommand,
    session::SessionCommand,
};
use coder::config::LlmConfig;
use coder::events::StreamEventWithMetadata;
use coder::session::{SessionManager, SessionManagerConfig};
use coder::utils;
use coder::utils::session::{create_default_session_config, create_session_store};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists
    coder::cli::config::load_env()?;

    // Initialize tracing (level configured via RUST_LOG env var)
    utils::tracing::init_tracing()?;

    // Set up signal handlers for terminal cleanup
    setup_signal_handlers().await;

    info!(target: "main", "Coder application starting");

    // Load or initialize config using the library path
    let _config = coder::config::load_config()?;

    // Handle subcommands if present
    if let Some(cmd) = &cli.command {
        return execute_command(cmd.clone(), &cli).await;
    }

    // Set working directory if specified
    if let Some(dir) = cli.directory {
        std::env::set_current_dir(dir)?;
    }

    match cli.remote {
        Some(remote_addr) => run_remote_mode(&remote_addr, cli.model).await,
        None => run_local_mode(cli.model).await,
    }
}

async fn execute_command(cmd: Commands, cli: &Cli) -> Result<()> {
    match cmd {
        Commands::Init { force } => {
            let command = InitCommand { force };
            command.execute().await
        }
        Commands::Headless {
            model,
            messages_json,
            timeout,
        } => {
            let command = HeadlessCommand {
                model,
                messages_json,
                timeout,
                global_model: cli.model,
            };
            command.execute().await
        }
        Commands::Serve { port, bind } => {
            let command = ServeCommand {
                port,
                bind,
                model: cli.model,
            };
            command.execute().await
        }
        Commands::Session { session_command } => {
            let command = SessionCommand {
                command: session_command,
                remote: cli.remote.clone(),
            };
            command.execute().await
        }
    }
}

async fn run_remote_mode(remote_addr: &str, model: Model) -> Result<()> {
    info!("Connecting to remote server at {}", remote_addr);

    // Set panic hook for terminal cleanup
    coder::tui::setup_panic_hook();

    // Create TUI in remote mode
    let (mut tui, event_rx) = coder::tui::Tui::new_remote(remote_addr, model, None).await?;

    println!("Connected to remote server at {}", remote_addr);

    // Run the TUI with events from the remote server
    tui.run(event_rx).await?;

    Ok(())
}

async fn run_local_mode(model: Model) -> Result<()> {
    let llm_config = LlmConfig::from_env()
        .expect("Failed to load LLM configuration from environment variables.");

    // Create session manager with SQLite store
    let session_store = create_session_store().await?;
    let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

    let session_manager_config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: model,
        auto_persist: true,
    };

    let session_manager =
        SessionManager::new(session_store, session_manager_config, global_event_tx);

    // Create a new interactive session
    let session_config = create_default_session_config();
    let app_config = AppConfig { llm_config };

    // Add the initial model to session metadata so it can be set in the App
    let mut session_config_with_model = session_config;
    session_config_with_model
        .metadata
        .insert("initial_model".to_string(), model.to_string());

    let (session_id, command_tx) = session_manager
        .create_session(session_config_with_model, app_config)
        .await
        .map_err(|e| anyhow!("Failed to create session: {}", e))?;

    // Get the event receiver
    let event_rx = session_manager
        .take_event_receiver(&session_id)
        .await
        .map_err(|e| anyhow!("Failed to get event receiver for new session: {}", e))?;

    info!(target: "main", "Created new session: {}", session_id);
    println!("Session ID: {}", session_id);

    // Set panic hook for terminal cleanup
    coder::tui::setup_panic_hook();

    // Create and run the TUI
    let mut tui = coder::tui::Tui::new(command_tx, model)?;
    tui.run(event_rx).await?;

    Ok(())
}

async fn setup_signal_handlers() {
    // Set up a flag to track terminal state for signal handlers
    let terminal_in_raw_mode = Arc::new(AtomicBool::new(false));
    let terminal_clone = terminal_in_raw_mode.clone();

    // Set up signal handler for SIGINT, SIGTERM
    #[cfg(not(windows))]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let sigterm_terminal = terminal_clone.clone();
        tokio::spawn(async move {
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
            sigterm.recv().await;

            // Clean up terminal if in raw mode
            if sigterm_terminal.load(Ordering::SeqCst) {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::LeaveAlternateScreen,
                    crossterm::event::DisableMouseCapture
                );
                info!(target: "signal_handler", "Received SIGTERM, terminal cleaned up");
            }

            std::process::exit(0);
        });

        let sigint_terminal = terminal_clone.clone();
        tokio::spawn(async move {
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");
            sigint.recv().await;

            // Clean up terminal if in raw mode
            if sigint_terminal.load(Ordering::SeqCst) {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::LeaveAlternateScreen,
                    crossterm::event::DisableMouseCapture
                );
                info!(target: "signal_handler", "Received SIGINT, terminal cleaned up");
            }

            std::process::exit(0);
        });
    }
}
