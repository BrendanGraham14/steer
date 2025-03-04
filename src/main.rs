use anyhow::Result;
use clap::{Parser, Subcommand};

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
    
    // Load or initialize config
    let config = config::load_config()?;
    
    // Check for API key
    let api_key = match cli.api_key.or(config.api_key) {
        Some(key) => key,
        None => {
            eprintln!("Error: No API key provided. Please use --api-key or set CLAUDE_API_KEY environment variable");
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
        // Add more configuration as needed
    };
    
    // Initialize the app
    let mut app = app::App::new(app_config)?;
    
    // Initialize the TUI
    let mut tui = tui::Tui::new()?;
    
    // Set up the event channel for app → TUI communication
    let event_rx = app.setup_event_channel();
    
    // Run the TUI with the app
    tui.run(&mut app, event_rx).await?;
    
    Ok(())
}
