use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use coder::api::{
    Model,
    messages::{Message, MessageContent, MessageRole},
};
use dotenv::dotenv;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

use coder::app::{App, AppCommand, AppConfig, app_actor_loop};
use coder::config::LlmConfig;
use coder::tui;
use coder::utils;

/// A command line tool to pair program with Claude
#[derive(Parser)]
#[command(version, about, long_about = None, author)]
struct Cli {
    /// Optional directory to work in
    #[arg(short, long)]
    directory: Option<std::path::PathBuf>,

    /// API Key for Claude (can also be set via CLAUDE_API_KEY env var)
    #[arg(short, long, env = "CLAUDE_API_KEY")]
    api_key: Option<String>,

    /// Model to use
    #[arg(short, long, value_enum, default_value_t = Model::Gemini2_5ProPreview0325)]
    model: Model,

    /// Subcommands
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new config file
    Init {
        /// Force overwrite of existing config
        #[arg(short, long)]
        force: bool,
    },
    /// Run in headless one-shot mode
    Headless {
        /// Model to use (overrides global setting)
        #[arg(long)]
        model: Option<Model>,

        /// JSON file containing a Vec<Message> to use. If not provided, reads prompt from stdin.
        #[arg(long)]
        messages_json: Option<PathBuf>,

        /// Timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists
    dotenv().ok();

    // Initialize tracing (level configured via RUST_LOG env var)
    utils::tracing::init_tracing()?;

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

    info!(target: "main", "Claude Code application starting");

    // Load or initialize config using the library path
    let _config = coder::config::load_config()?;

    // Handle subcommands if present
    if let Some(cmd) = cli.command {
        match cmd {
            Commands::Init { force } => {
                // Use library path for config functions
                coder::config::init_config(force)?;
                println!("Configuration initialized successfully.");
                return Ok(());
            }
            Commands::Headless {
                model,
                messages_json,
                timeout,
            } => {
                // Parse input into Vec<Message>
                let messages = if let Some(json_path) = messages_json {
                    // Read messages from JSON file
                    let json_content = fs::read_to_string(json_path)
                        .map_err(|e| anyhow!("Failed to read messages JSON file: {}", e))?;

                    serde_json::from_str::<Vec<Message>>(&json_content)
                        .map_err(|e| anyhow!("Failed to parse messages JSON: {}", e))?
                } else {
                    // Read prompt from stdin
                    let mut buffer = String::new();
                    match io::stdin().read_to_string(&mut buffer) {
                        Ok(_) => {
                            if buffer.trim().is_empty() {
                                return Err(anyhow!("No input provided via stdin"));
                            }
                        }
                        Err(e) => return Err(anyhow!("Failed to read from stdin: {}", e)),
                    }
                    // Create a single user message from stdin content
                    vec![Message {
                        role: MessageRole::User,
                        content: MessageContent::Text { content: buffer },
                        id: None,
                    }]
                };

                // Set up timeout if provided
                let timeout_duration = timeout.map(|secs| Duration::from_secs(secs));

                // Use model override if provided, otherwise use the global setting
                let model_to_use = model.unwrap_or(cli.model);

                let llm_config = LlmConfig::from_env()
                    .expect("Failed to load LLM configuration from environment variables.");

                // Run the agent in one-shot mode
                let result =
                    coder::run_once(messages, model_to_use, &llm_config, timeout_duration).await?;

                // Output the result as JSON
                let json_output = serde_json::to_string_pretty(&result)
                    .map_err(|e| anyhow!("Failed to serialize result to JSON: {}", e))?;

                println!("{}", json_output);
                return Ok(());
            }
        }
    }

    if let Some(dir) = cli.directory {
        std::env::set_current_dir(dir)?;
    }

    let llm_config = LlmConfig::from_env()
        .expect("Failed to load LLM configuration from environment variables.");

    let (app_command_tx, app_command_rx) = mpsc::channel::<AppCommand>(32);
    let (app_event_tx, app_event_rx) = mpsc::channel(32);

    // Initialize the global command sender for tool approval requests
    coder::app::OpContext::init_command_tx(app_command_tx.clone());

    let app_config = AppConfig { llm_config };
    let app = App::new(app_config, app_event_tx, cli.model)?;

    tokio::spawn(app_actor_loop(app, app_command_rx));
    tui::run_tui(app_command_tx, app_event_rx, cli.model).await?;

    Ok(())
}
