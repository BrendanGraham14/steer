use anyhow::Result;
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use std::panic;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{self, Receiver};

mod api;
mod app;
mod config;
mod tools;
mod tui;
mod utils;

use app::AppCommand;

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
    /// Clear conversation history
    Clear,
    /// Compact the conversation to save context space
    Compact,
    /// Show detailed help information
    Info,
}

async fn app_actor_loop(
    mut app: app::App,
    mut command_rx: Receiver<AppCommand>,
    mut internal_event_rx: Receiver<app::AppEvent>,
) -> Result<()> {
    utils::logging::info("app_actor_loop", "App actor loop started.");
    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                utils::logging::debug("app_actor_loop", &format!("Received command: {:?}", command));
                match command {
                    AppCommand::ProcessUserInput(input) => {
                        if let Err(e) = app.process_user_message(input).await {
                             utils::logging::error("app_actor_loop", &format!("Error processing user input: {}", e));
                             app.emit_event(app::AppEvent::Error { message: e.to_string() });
                        }
                    }
                    AppCommand::HandleToolResponse { id, approved, always } => {
                        if let Err(e) = app.handle_tool_command_response(id, approved, always).await {
                            utils::logging::error("app_actor_loop", &format!("Error handling tool command response: {}", e));
                            app.emit_event(app::AppEvent::Error { message: e.to_string() });
                        }
                    }
                    AppCommand::ExecuteCommand(cmd) => {
                         if let Err(e) = app.handle_command(&cmd).await {
                              utils::logging::error("app_actor_loop", &format!("Error executing command: {}", e));
                         }
                    }
                    AppCommand::Shutdown => {
                        utils::logging::info("app_actor_loop", "Shutdown command received.");
                        break;
                    }
                }
            },
            Some(internal_event) = internal_event_rx.recv() => {
                 utils::logging::debug("app_actor_loop", &format!("Received internal event: {:?}", internal_event));
                 match internal_event {
                     app::AppEvent::ToolBatchProgress { batch_id } => {
                         if let Err(e) = app.handle_batch_progress(batch_id).await {
                             utils::logging::error("app_actor_loop", &format!("Error handling batch progress: {}", e));
                         }
                     }
                     _ => {
                        utils::logging::warn("app_actor_loop", &format!("Unhandled internal event: {:?}", internal_event));
                     }
                 }
            },
            else => {
                utils::logging::info("app_actor_loop", "Command channel closed or loop broken.");
                break;
            }
        }
    }
    utils::logging::info("app_actor_loop", "App actor loop finished.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists
    dotenv().ok();

    // Initialize logging
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
                    crossterm::terminal::LeaveAlternateScreen
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
                    crossterm::terminal::LeaveAlternateScreen
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

    // Load or initialize config
    let config = config::load_config()?;

    // Check for API key in order: CLI arg > env var (including .env) > config file
    let api_key = match cli
        .api_key
        .or_else(|| std::env::var("CLAUDE_API_KEY").ok())
        .or(config.api_key)
    {
        Some(key) => key,
        None => {
            eprintln!(
                "Error: No API key provided. Please use --api-key, set CLAUDE_API_KEY environment variable, add it to .env file, or configure it in config file"
            );
            std::process::exit(1);
        }
    };

    // Handle subcommands if present
    if let Some(cmd) = cli.command {
        match cmd {
            Commands::Init { force } => {
                config::init_config(force)?;
                println!("Configuration initialized successfully.");
                return Ok(());
            }
            Commands::Clear => {
                // TODO: Clear conversation history
                println!("Conversation history cleared.");
                return Ok(());
            }
            Commands::Compact => {
                // TODO: Implement conversation compaction
                println!("Compacting conversation is not yet implemented.");
                return Ok(());
            }
            Commands::Info => {
                // TODO: Implement detailed help
                println!(
                    "Detailed information not yet implemented. Use --help for basic commands."
                );
                return Ok(());
            }
        }
    }

    // Set working directory if specified
    if let Some(dir) = cli.directory {
        std::env::set_current_dir(dir)?;
    }

    // --- App Initialization ---
    let app_config = app::AppConfig {
        api_key,
        // Add more configuration options if needed in the future
    };

    utils::logging::info("main", "Initializing application");

    // Create Event Channel first
    let (event_tx, event_rx) = mpsc::channel(100);

    // Create Internal Event Channel (App -> Actor Loop for things like batch progress)
    let (internal_event_tx, internal_event_rx) = mpsc::channel(100);

    // Create Command Channel
    let (command_tx, command_rx) = mpsc::channel::<AppCommand>(100);

    // Instantiate App - Pass channel senders
    let mut app = match app::App::new(app_config, event_tx.clone(), internal_event_tx.clone()) {
        Ok(app) => app,
        Err(e) => {
            utils::logging::error("main", &format!("Failed to initialize app: {}", e));
            eprintln!("Error: Failed to initialize application: {}", e);
            return Err(e);
        }
    };

    // Spawn the App Actor Task
    utils::logging::info("main", "Spawning App actor task");
    let _app_handle = tokio::spawn(app_actor_loop(app, command_rx, internal_event_rx));

    // Set up panic hook to ensure terminal is reset if the app crashes
    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let terminal_result = crossterm::terminal::disable_raw_mode();
        let screen_result =
            crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

        utils::logging::error(
            "panic_hook",
            &format!("Application panicked: {}", panic_info),
        );

        if let Err(e) = terminal_result {
            utils::logging::error("panic_hook", &format!("Failed to disable raw mode: {}", e));
        }

        if let Err(e) = screen_result {
            utils::logging::error(
                "panic_hook",
                &format!("Failed to leave alternate screen: {}", e),
            );
        }

        eprintln!("\nERROR: Application crashed: {}", panic_info);

        orig_hook(panic_info);
    }));

    // --- TUI Initialization ---
    utils::logging::info("main", "Initializing TUI");
    let mut tui = match tui::Tui::new(command_tx.clone()) {
        Ok(tui) => tui,
        Err(e) => {
            utils::logging::error("main", &format!("Failed to initialize TUI: {}", e));
            eprintln!("Error: Failed to initialize terminal UI: {}", e);
            return Err(e);
        }
    };

    // Mark that terminal is now in raw mode
    terminal_in_raw_mode.store(true, Ordering::SeqCst);

    // Run the TUI, passing only the event receiver
    utils::logging::info("main", "Starting TUI task");
    let tui_handle = tokio::task::spawn(async move { tui.run(event_rx).await });

    // --- Wait for Tasks ---
    utils::logging::info("main", "Waiting for TUI and App tasks to complete");

    // Wait for the TUI task to complete first
    let tui_result = tui_handle.await?;

    // Mark terminal as no longer in raw mode (important after TUI finishes)
    terminal_in_raw_mode.store(false, Ordering::SeqCst);

    // Handle TUI result
    match tui_result {
        Ok(_) => {
            utils::logging::info("main", "TUI terminated normally");
        }
        Err(e) => {
            utils::logging::error("main", &format!("TUI task error: {}", e));
            eprintln!("Error in TUI: {}", e);
            return Err(e.into());
        }
    }

    utils::logging::info("main", "Application shutting down");
    Ok(())
}
