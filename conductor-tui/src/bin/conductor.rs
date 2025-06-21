use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use conductor_tui::tui::cleanup_terminal;

use conductor_core::api::Model;
use conductor_core::app::AppConfig;
use conductor_core::app::io::AppEventSource;
use conductor::cli::{Cli, Commands};
use conductor::commands::{
    Command, headless::HeadlessCommand, init::InitCommand, serve::ServeCommand,
    session::SessionCommand,
};
use conductor_core::config::LlmConfig;
use conductor_core::events::StreamEventWithMetadata;
use conductor_core::utils;
use conductor_core::utils::session::{create_default_session_config, create_session_store};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists
    conductor::cli::config::load_env()?;

    // Initialize tracing (level configured via RUST_LOG env var)
    utils::tracing::init_tracing()?;

    // Set up signal handlers for terminal cleanup
    setup_signal_handlers().await;

    info!(target: "main", "Conductor application starting (TUI)");

    // Load or initialize config
    let _config = conductor_core::config::load_config()?;

    // Handle subcommands if present
    if let Some(cmd) = &cli.command {
        return execute_command(cmd.clone(), &cli).await;
    }

    // Set working directory if specified
    if let Some(dir) = cli.directory {
        std::env::set_current_dir(dir)?;
    }

    match &cli.remote {
        Some(remote_addr) => run_remote_mode(remote_addr, cli.model, cli.system_prompt).await,
        None => run_local_mode(cli.model, cli.system_prompt).await,
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
            session,
            tool_config,
            system_prompt,
        } => {
            let command = HeadlessCommand {
                model,
                messages_json,
                global_model: cli.model,
                session,
                tool_config,
                system_prompt,
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

async fn run_remote_mode(
    remote_addr: &str,
    model: Model,
    system_prompt: Option<String>,
) -> Result<()> {
    info!("Connecting to remote server at {}", remote_addr);

    // Set panic hook for terminal cleanup
    conductor_tui::tui::setup_panic_hook();

    // Create TUI in remote mode
    let session_config = system_prompt.map(|prompt| {
        let mut config = create_default_session_config();
        config.system_prompt = Some(prompt);
        config
    });
    let (mut tui, event_rx) = conductor_tui::tui::Tui::new_remote(remote_addr, model, session_config).await?;

    println!("Connected to remote server at {}", remote_addr);

    // Run the TUI with events from the remote server
    tui.run(event_rx).await?;

    Ok(())
}

async fn run_local_mode(model: Model, system_prompt: Option<String>) -> Result<()> {
    let llm_config = LlmConfig::from_env()
        .expect("Failed to load LLM configuration from environment variables.");

    info!(target: "main", "Starting local mode with in-memory gRPC");

    // Set up the in-memory gRPC server and get a channel
    let channel = conductor_grpc::in_memory::setup_local_grpc(llm_config, model).await?;

    // Now use the same remote mode code path but with the in-memory channel
    // Create a fake address for logging purposes
    let addr = "in-memory://localhost";
    
    // Set panic hook for terminal cleanup
    conductor_tui::tui::setup_panic_hook();

    // Create session config with system prompt if provided
    let session_config = system_prompt.map(|prompt| {
        let mut config = create_default_session_config();
        config.system_prompt = Some(prompt);
        config
    });

    // Use the new_remote method but with our in-memory channel
    // We need to modify new_remote to accept an optional channel
    // For now, let's create the client directly
    use conductor_grpc::GrpcClientAdapter;
    use conductor_core::session::{SessionConfig, SessionToolConfig};
    use conductor_core::app::io::AppEventSource;
    use std::collections::HashMap;

    info!("Creating gRPC client for in-memory channel");

    // Create client from the in-memory channel
    let mut client = GrpcClientAdapter::from_channel(channel).await?;

    // Create or use provided session config
    let session_config = session_config.unwrap_or_else(|| SessionConfig {
        workspace: conductor_core::session::state::WorkspaceConfig::default(),
        tool_config: SessionToolConfig::default(),
        system_prompt: None,
        metadata: HashMap::new(),
    });

    // Add the initial model to session metadata
    let mut session_config_with_model = session_config;
    session_config_with_model
        .metadata
        .insert("initial_model".to_string(), model.to_string());

    // Create session on server
    let session_id = client.create_session(session_config_with_model).await?;
    info!("Created local session via gRPC: {}", session_id);
    println!("Session ID: {}", session_id);

    // Start streaming
    client.start_streaming().await?;

    // Get the event receiver before wrapping in Arc
    let event_rx = client.subscribe().await;

    // Wrap client in Arc
    let client = Arc::new(client);

    // Initialize TUI with the gRPC client as command sink
    let mut tui = conductor_tui::tui::Tui::new(client, model)?;

    // Run the TUI
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
        use tokio::signal::unix::{signal, SignalKind};

        let sigterm_terminal = terminal_clone.clone();
        tokio::spawn(async move {
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
        tokio::spawn(async move {
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
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            if windows_terminal.load(Ordering::Relaxed) {
                cleanup_terminal();
            }
            std::process::exit(130);
        });
    }
}

// Re-export setup_panic_hook so it can be used by main
pub use conductor_tui::tui::setup_panic_hook;