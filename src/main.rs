use anyhow::Result;
use clap::{Parser, Subcommand};
use coder::api::Model;
use dotenv::dotenv;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

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
    #[arg(short, long, value_enum, default_value_t = Model::Claude3_7Sonnet20250219)]
    model: Model,

    /// Enable debug logging to file
    #[arg(long)]
    debug: bool,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists
    dotenv().ok();

    utils::logging::init_logging()?;

    // Set log level based on debug flag
    if !cli.debug {
        if let Ok(mut logger) = utils::logging::Logger::get().lock() {
            logger.set_level(utils::logging::LogLevel::Info);
        }
    }

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
                utils::logging::info("signal_handler", "Received SIGTERM, terminal cleaned up");
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
                utils::logging::info("signal_handler", "Received SIGINT, terminal cleaned up");
            }

            std::process::exit(0);
        });
    }

    utils::logging::info("main", "Claude Code application starting");
    if cli.debug {
        utils::logging::info("main", "Debug logging enabled");
    }

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
        }
    }

    if let Some(dir) = cli.directory {
        std::env::set_current_dir(dir)?;
    }

    let llm_config = LlmConfig::from_env()
        .expect("Failed to load LLM configuration from environment variables.");

    let (app_command_tx, app_command_rx) = mpsc::channel::<AppCommand>(32);
    let (app_event_tx, app_event_rx) = mpsc::channel(32);

    let app_config = AppConfig { llm_config };
    let app = App::new(app_config, app_event_tx, cli.model)?;

    tokio::spawn(app_actor_loop(app, app_command_rx));
    tui::run_tui(app_command_tx, app_event_rx, cli.model).await?;

    Ok(())
}
