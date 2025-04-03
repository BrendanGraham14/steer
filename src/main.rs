use anyhow::Result;
use clap::{Parser, Subcommand};
use dotenv::dotenv;

mod app;
mod api;
mod config;
mod tui;
mod tools;
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
    
    // Initialize logging system with appropriate level based on debug flag
    let log_level = if cli.debug {
        utils::logging::LogLevel::Debug
    } else {
        utils::logging::LogLevel::Info
    };
    
    // Create a timestamped log file
    let now = chrono::Local::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    let home = dirs::home_dir();
    let log_path = home.map(|h| h.join(".claude-code").join(format!("{}.log", timestamp)));
    
    // Create the directory if it doesn't exist
    if let Some(path) = &log_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    
    // Initialize logger
    utils::logging::Logger::init(log_path.as_deref(), log_level)?;
    
    utils::logging::info("main", "Claude Code application starting");
    if cli.debug {
        utils::logging::info("main", "Debug logging enabled");
    }
    
    // Load .env file if it exists
    dotenv().ok();
    
    // Load or initialize config
    let config = config::load_config()?;
    
    // Check for API key in order: CLI arg > env var (including .env) > config file
    let api_key = match cli.api_key.or_else(|| std::env::var("CLAUDE_API_KEY").ok()).or(config.api_key) {
        Some(key) => key,
        None => {
            eprintln!("Error: No API key provided. Please use --api-key, set CLAUDE_API_KEY environment variable, add it to .env file, or configure it in config file");
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
                println!("Detailed information not yet implemented. Use --help for basic commands.");
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
    
    // Set up the event channel for app → TUI communication
    utils::logging::info("main", "Setting up event channel");
    let event_rx = app.setup_event_channel();
    
    // Run the TUI with the app
    utils::logging::info("main", "Starting application main loop");
    match tui.run(&mut app, event_rx).await {
        Ok(_) => {
            utils::logging::info("main", "Application terminated normally");
        },
        Err(e) => {
            utils::logging::error("main", &format!("Application error: {}", e));
            eprintln!("Error: {}", e);
            return Err(e);
        }
    }
    
    Ok(())
}
