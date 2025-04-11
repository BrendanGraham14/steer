use anyhow::Result;
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use std::panic;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod api;
mod app;
mod config;
mod tools;
mod tui;
mod utils;

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
        use tokio::signal::unix::{signal, SignalKind};
        
        let sigterm_terminal = terminal_clone.clone();
        tokio::spawn(async move {
            let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");
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
            let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");
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

    // Start the TUI app
    let app_config = app::AppConfig {
        api_key,
        // Add more configuration options if needed in the future
    };

    // Initialize the app
    utils::logging::info("main", "Initializing application");
    let mut app = match app::App::new(app_config) {
        Ok(app) => app,
        Err(e) => {
            utils::logging::error("main", &format!("Failed to initialize app: {}", e));
            eprintln!("Error: Failed to initialize application: {}", e);
            return Err(e);
        }
    };

    // Set up panic hook to ensure terminal is reset if the app crashes
    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // Always attempt to disable raw mode on panic
        let terminal_result = crossterm::terminal::disable_raw_mode();
        let screen_result = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen
        );
        
        // Log the panic and any terminal cleanup failures
        utils::logging::error("panic_hook", &format!("Application panicked: {}", panic_info));
        
        if let Err(e) = terminal_result {
            utils::logging::error("panic_hook", &format!("Failed to disable raw mode: {}", e));
        }
        
        if let Err(e) = screen_result {
            utils::logging::error("panic_hook", &format!("Failed to leave alternate screen: {}", e));
        }
        
        // Print error to stderr
        eprintln!("\nERROR: Application crashed: {}", panic_info);
        
        // Call the original panic handler
        orig_hook(panic_info);
    }));

    // Initialize the TUI
    utils::logging::info("main", "Initializing TUI");
    let mut tui = match tui::Tui::new() {
        Ok(tui) => tui,
        Err(e) => {
            utils::logging::error("main", &format!("Failed to initialize TUI: {}", e));
            eprintln!("Error: Failed to initialize terminal UI: {}", e);
            return Err(e);
        }
    };
    
    // Mark that terminal is now in raw mode
    terminal_in_raw_mode.store(true, Ordering::SeqCst);

    // Set up the event channel for app → TUI communication
    utils::logging::info("main", "Setting up event channel");
    let event_rx = app.setup_event_channel();

    // Run the TUI with the app
    utils::logging::info("main", "Starting application main loop");
    let result = tui.run(&mut app, event_rx).await;
    
    // Mark terminal as no longer in raw mode
    terminal_in_raw_mode.store(false, Ordering::SeqCst);
    
    match result {
        Ok(_) => {
            utils::logging::info("main", "Application terminated normally");
        }
        Err(e) => {
            utils::logging::error("main", &format!("Application error: {}", e));
            eprintln!("Error: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
