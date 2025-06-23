use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;
use conductor_tui::tui::cleanup_terminal;

use conductor_core::api::Model;
use conductor::cli::{Cli, Commands};
use conductor::commands::{
    Command, headless::HeadlessCommand, init::InitCommand, serve::ServeCommand,
    session::SessionCommand,
};
use conductor_core::config::LlmConfig;
use conductor_core::utils;

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

    // Set panic hook for terminal cleanup
    conductor_tui::tui::setup_panic_hook();

    // Create the gRPC client (remote or local)
    use conductor_grpc::GrpcClientAdapter;
    
    let client = match &cli.remote {
        Some(remote_addr) => {
            info!("Connecting to remote server at {}", remote_addr);
            Arc::new(GrpcClientAdapter::connect(remote_addr).await?)
        }
        None => {
            let llm_config = LlmConfig::from_env()
                .expect("Failed to load LLM configuration from environment variables.");
            info!(target: "main", "Starting local mode with in-memory gRPC");
            let channel = conductor_grpc::in_memory::setup_local_grpc(llm_config, cli.model).await?;
            Arc::new(GrpcClientAdapter::from_channel(channel).await?)
        }
    };

    // Either resume existing session (explicit id or "latest") or create new one
    let session_arg = cli.session.clone();

    let session_id_to_resume = if let Some(ref s) = session_arg {
        if s == "latest" {
            Some(fetch_latest_session_id(&client).await?)
        } else {
            Some(s.clone())
        }
    } else {
        None
    };

    if let Some(session_id) = session_id_to_resume {
        resume_session(client, session_id, cli.model).await
    } else {
        run_new_session(client, cli.model, cli.system_prompt).await
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

async fn fetch_latest_session_id(client: &Arc<conductor_grpc::GrpcClientAdapter>) -> Result<String> {
    let sessions = client.list_sessions().await?;
    let latest = sessions
        .into_iter()
        .max_by_key(|s| s.updated_at.as_ref().map(|ts| ts.seconds).unwrap_or(0))
        .ok_or_else(|| anyhow!("No sessions found"))?;
    Ok(latest.id)
}

async fn resume_session(
    client: Arc<conductor_grpc::GrpcClientAdapter>,
    session_id: String,
    model: Model,
) -> Result<()> {
    use conductor_core::app::io::AppEventSource;
    
    // Activate the existing session
    let client_ref = Arc::clone(&client);
    let (messages, approved_tools) = client_ref.activate_session(session_id.clone()).await?;
    info!("Activated session: {} with {} messages", session_id, messages.len());
    println!("Session ID: {}", session_id);

    // Start streaming
    client_ref.start_streaming().await?;

    // Get the event receiver
    let event_rx = client_ref.subscribe().await;

    // Initialize TUI with the gRPC client as command sink and restored messages
    let mut tui = conductor_tui::tui::Tui::new_with_conversation(client, model, messages, approved_tools)?;

    // Run the TUI
    tui.run(event_rx).await?;

    Ok(())
}

async fn run_new_session(
    client: Arc<conductor_grpc::GrpcClientAdapter>,
    model: Model,
    system_prompt: Option<String>,
) -> Result<()> {
    use conductor_core::session::{SessionConfig, SessionToolConfig};
    use conductor_core::app::io::AppEventSource;
    use std::collections::HashMap;

    // Create session config
    let mut session_config = SessionConfig {
        workspace: conductor_core::session::state::WorkspaceConfig::default(),
        tool_config: SessionToolConfig::default(),
        system_prompt,
        metadata: HashMap::new(),
    };

    // Add the initial model to session metadata
    session_config
        .metadata
        .insert("initial_model".to_string(), model.to_string());

    // Create session on server
    let session_id = client.create_session(session_config).await?;
    info!("Created session: {}", session_id);
    println!("Session ID: {}", session_id);

    // Start streaming
    client.start_streaming().await?;

    // Get the event receiver
    let event_rx = client.subscribe().await;

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