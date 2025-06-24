use anyhow::Result;
use clap::Parser;

use conductor::cli::{Cli, Commands};
use conductor::commands::{
    Command, headless::HeadlessCommand, init::InitCommand, serve::ServeCommand,
    session::SessionCommand,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load .env file if it exists
    conductor::cli::config::load_env()?;

    // Initialize tracing (level configured via RUST_LOG env var)
    conductor::utils::tracing::init_tracing()?;

    // Handle subcommands if present
    if let Some(cmd) = &cli.command {
        return execute_command(cmd.clone(), &cli).await;
    }

    // If no subcommand, inform user to use conductor-tui for interactive mode
    println!("conductor-core provides the core functionality for Conductor.");
    println!();
    println!("For the interactive Terminal UI, please use the 'conductor' command");
    println!("provided by the conductor-tui package.");
    println!();
    println!("Available subcommands:");
    println!("  init       - Initialize configuration");
    println!("  headless   - Run in headless mode");
    println!("  serve      - Start gRPC server");
    println!("  session    - Manage sessions");
    println!();
    println!("Run 'conductor-core --help' for more information.");

    Ok(())
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
