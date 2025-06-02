use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use coder::api::{
    Model,
    messages::{Message, MessageContent, MessageRole},
};
use dotenv::dotenv;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

use coder::app::{App, AppCommand, AppConfig, app_actor_loop};
use coder::config::LlmConfig;
use coder::events::StreamEventWithMetadata;
use coder::session::stores::sqlite::SqliteSessionStore;
use coder::session::{
    SessionConfig, SessionManager, SessionManagerConfig, SessionToolConfig, ToolApprovalPolicy,
};
use coder::utils;

/// An AI-powered agent and CLI tool that assists with software engineering tasks.
#[derive(Parser)]
#[command(version, about, long_about = None, author)]
struct Cli {
    /// Optional directory to work in
    #[arg(short, long)]
    directory: Option<std::path::PathBuf>,

    /// Model to use
    #[arg(short, long, value_enum, default_value_t = Model::ClaudeSonnet4_20250514)]
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
    /// Session management commands
    Session {
        #[command(subcommand)]
        session_command: SessionCommands,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    List {
        /// Show only active sessions
        #[arg(long)]
        active: bool,
        /// Limit number of sessions to show
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Create a new session
    Create {
        /// Tool approval policy (always_ask, pre_approved, mixed)
        #[arg(long, default_value = "always_ask")]
        tool_policy: String,
        /// Pre-approved tools (comma-separated)
        #[arg(long)]
        pre_approved_tools: Option<String>,
        /// Session metadata (key=value pairs, comma-separated)
        #[arg(long)]
        metadata: Option<String>,
    },
    /// Resume an existing session
    Resume {
        /// Session ID to resume
        session_id: String,
    },
    /// Resume the latest (most recently updated) session
    Latest,
    /// Delete a session
    Delete {
        /// Session ID to delete
        session_id: String,
        /// Force deletion without confirmation
        #[arg(long)]
        force: bool,
    },
    /// Show session details
    Show {
        /// Session ID to show
        session_id: String,
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

    info!(target: "main", "Coder application starting");

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
            Commands::Session { session_command } => {
                let llm_config = LlmConfig::from_env()
                    .expect("Failed to load LLM configuration from environment variables.");

                return handle_session_command(session_command).await;
            }
        }
    }

    if let Some(dir) = cli.directory {
        std::env::set_current_dir(dir)?;
    }

    let llm_config = LlmConfig::from_env()
        .expect("Failed to load LLM configuration from environment variables.");

    // Create session manager with SQLite store
    let session_store = create_session_store().await?;
    let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

    let session_manager_config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: cli.model,
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
        .insert("initial_model".to_string(), cli.model.to_string());

    let (session_id, command_tx) = session_manager
        .create_session(session_config_with_model, app_config)
        .await
        .map_err(|e| anyhow!("Failed to create session: {}", e))?;

    // Get the event receiver
    let event_rx = session_manager
        .take_event_receiver(&session_id)
        .await
        .ok_or_else(|| anyhow!("Failed to get event receiver for new session"))?;

    info!(target: "main", "Created new session: {}", session_id);
    println!("Session ID: {}", session_id);

    // Set panic hook for terminal cleanup
    coder::tui::setup_panic_hook();

    // Create and run the TUI
    let mut tui = coder::tui::Tui::new(command_tx, cli.model)?;
    tui.run(event_rx).await?;

    Ok(())
}

async fn handle_session_command(command: SessionCommands) -> Result<()> {
    let session_store = create_session_store().await?;
    let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

    let session_manager_config = SessionManagerConfig {
        max_concurrent_sessions: 10,
        default_model: Model::ClaudeSonnet4_20250514,
        auto_persist: true,
    };

    let session_manager =
        SessionManager::new(session_store, session_manager_config, global_event_tx);

    match command {
        SessionCommands::List { active, limit } => {
            let filter = coder::session::SessionFilter {
                status_filter: if active {
                    Some(coder::session::SessionStatus::Active)
                } else {
                    None
                },
                limit,
                ..Default::default()
            };

            let sessions = session_manager
                .list_sessions(filter)
                .await
                .map_err(|e| anyhow!("Failed to list sessions: {}", e))?;

            if sessions.is_empty() {
                println!("No sessions found.");
                return Ok(());
            }

            println!("Sessions:");
            println!(
                "{:<36} {:<20} {:<20} {:<10} {:<30}",
                "ID", "Created", "Updated", "Messages", "Last Model"
            );
            println!("{}", "-".repeat(106));

            for session in sessions {
                let model_str = session
                    .last_model
                    .map(|m| m.as_ref().to_string())
                    .unwrap_or_else(|| "N/A".to_string());

                println!(
                    "{:<36} {:<20} {:<20} {:<10} {:<30}",
                    session.id,
                    session.created_at.format("%Y-%m-%d %H:%M:%S"),
                    session.updated_at.format("%Y-%m-%d %H:%M:%S"),
                    session.message_count,
                    model_str,
                );
            }
        }
        SessionCommands::Create {
            tool_policy,
            pre_approved_tools,
            metadata,
        } => {
            let policy = parse_tool_policy(&tool_policy, pre_approved_tools.as_deref())?;
            let session_metadata = parse_metadata(metadata.as_deref())?;

            let session_config = SessionConfig {
                tool_policy: policy,
                tool_config: SessionToolConfig::default(),
                metadata: session_metadata,
            };

            let app_config = AppConfig {
                llm_config: LlmConfig::from_env()?,
            };

            let (session_id, _) = session_manager
                .create_session(session_config, app_config)
                .await
                .map_err(|e| anyhow!("Failed to create session: {}", e))?;

            println!("Created session: {}", session_id);
        }
        SessionCommands::Resume { session_id } => {
            // Resume the session in the TUI directly
            println!("Resuming session: {}", session_id);

            let llm_config = LlmConfig::from_env()?;

            // Create session manager with SQLite store
            let session_store = create_session_store().await?;
            let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

            let session_manager_config = SessionManagerConfig {
                max_concurrent_sessions: 10,
                default_model: Model::ClaudeSonnet4_20250514,
                auto_persist: true,
            };

            let session_manager =
                SessionManager::new(session_store, session_manager_config, global_event_tx);

            // Resume the session
            let app_config = AppConfig { llm_config };

            match session_manager
                .resume_session(&session_id, app_config)
                .await
            {
                Ok((true, command_tx)) => {
                    // Get the event receiver
                    let event_rx = session_manager
                        .take_event_receiver(&session_id)
                        .await
                        .ok_or_else(|| {
                            anyhow!("Failed to get event receiver for resumed session")
                        })?;

                    // Get the session info to determine the model
                    let session_info = session_manager
                        .get_session(&session_id)
                        .await?
                        .ok_or_else(|| anyhow!("Session not found after resume"))?;

                    let model = session_info
                        .last_model
                        .unwrap_or(Model::ClaudeSonnet4_20250514);

                    // Set panic hook for terminal cleanup
                    coder::tui::setup_panic_hook();

                    // Create and run the TUI
                    let mut tui = coder::tui::Tui::new(command_tx, model)?;
                    tui.run(event_rx).await?;
                }
                Ok((false, _)) => {
                    return Err(anyhow!("Session {} not found", session_id));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to resume session: {}", e));
                }
            }
        }
        SessionCommands::Latest => {
            // Get the most recently updated session
            let filter = coder::session::SessionFilter {
                order_by: coder::session::SessionOrderBy::UpdatedAt,
                order_direction: coder::session::OrderDirection::Descending,
                limit: Some(1),
                ..Default::default()
            };

            let sessions = session_manager
                .list_sessions(filter)
                .await
                .map_err(|e| anyhow!("Failed to list sessions: {}", e))?;

            if sessions.is_empty() {
                return Err(anyhow!("No sessions found"));
            }

            let latest_session = &sessions[0];
            let session_id = &latest_session.id;

            println!("Resuming latest session: {}", session_id);
            println!(
                "Last updated: {}",
                latest_session.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
            );

            // Resume the session in the TUI directly
            let llm_config = LlmConfig::from_env()?;

            // Create session manager with SQLite store
            let session_store = create_session_store().await?;
            let (global_event_tx, _global_event_rx) = mpsc::channel::<StreamEventWithMetadata>(100);

            let session_manager_config = SessionManagerConfig {
                max_concurrent_sessions: 10,
                default_model: Model::ClaudeSonnet4_20250514,
                auto_persist: true,
            };

            let session_manager =
                SessionManager::new(session_store, session_manager_config, global_event_tx);

            // Resume the session
            let app_config = AppConfig { llm_config };

            match session_manager.resume_session(session_id, app_config).await {
                Ok((true, command_tx)) => {
                    // Get the event receiver
                    let event_rx = session_manager
                        .take_event_receiver(session_id)
                        .await
                        .ok_or_else(|| {
                            anyhow!("Failed to get event receiver for resumed session")
                        })?;

                    let model = latest_session
                        .last_model
                        .unwrap_or(Model::ClaudeSonnet4_20250514);

                    // Set panic hook for terminal cleanup
                    coder::tui::setup_panic_hook();

                    // Create and run the TUI
                    let mut tui = coder::tui::Tui::new(command_tx, model)?;
                    tui.run(event_rx).await?;
                }
                Ok((false, _)) => {
                    return Err(anyhow!("Session {} not found", session_id));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to resume session: {}", e));
                }
            }
        }
        SessionCommands::Delete { session_id, force } => {
            if !force {
                print!(
                    "Are you sure you want to delete session {}? (y/N): ",
                    session_id
                );
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;

                if !input.trim().to_lowercase().starts_with('y') {
                    println!("Deletion cancelled.");
                    return Ok(());
                }
            }

            let deleted = session_manager
                .delete_session(&session_id)
                .await
                .map_err(|e| anyhow!("Failed to delete session: {}", e))?;

            if deleted {
                println!("Session {} deleted.", session_id);
            } else {
                return Err(anyhow!("Session not found: {}", session_id));
            }
        }
        SessionCommands::Show { session_id } => {
            let session_info = session_manager
                .get_session(&session_id)
                .await
                .map_err(|e| anyhow!("Failed to get session: {}", e))?;

            match session_info {
                Some(info) => {
                    println!("Session Details:");
                    println!("ID: {}", info.id);
                    println!(
                        "Created: {}",
                        info.created_at.format("%Y-%m-%d %H:%M:%S UTC")
                    );
                    println!(
                        "Updated: {}",
                        info.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
                    );
                    println!("Messages: {}", info.message_count);
                    println!(
                        "Last Model: {}",
                        info.last_model
                            .map(|m| m.as_ref().to_string())
                            .unwrap_or_else(|| "N/A".to_string())
                    );

                    if !info.metadata.is_empty() {
                        println!("Metadata:");
                        for (key, value) in &info.metadata {
                            println!("  {}: {}", key, value);
                        }
                    }
                }
                None => {
                    return Err(anyhow!("Session not found: {}", session_id));
                }
            }
        }
    }

    Ok(())
}

async fn create_session_store() -> Result<Arc<dyn coder::session::SessionStore>> {
    // Create SQLite session store in user's home directory
    let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("Could not determine home directory"))?;

    let db_path = home_dir.join(".coder").join("sessions.db");

    // Create directory if it doesn't exist
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create sessions directory: {}", e))?;
    }

    let store = SqliteSessionStore::new(&db_path)
        .await
        .map_err(|e| anyhow!("Failed to create session store: {}", e))?;

    Ok(Arc::new(store))
}

fn create_default_session_config() -> SessionConfig {
    SessionConfig {
        tool_policy: ToolApprovalPolicy::AlwaysAsk,
        tool_config: SessionToolConfig::default(),
        metadata: std::collections::HashMap::new(),
    }
}

fn parse_tool_policy(
    policy_str: &str,
    pre_approved_tools: Option<&str>,
) -> Result<ToolApprovalPolicy> {
    match policy_str {
        "always_ask" => Ok(ToolApprovalPolicy::AlwaysAsk),
        "pre_approved" => {
            let tools = if let Some(tools_str) = pre_approved_tools {
                tools_str.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                return Err(anyhow!(
                    "pre_approved_tools is required when using pre_approved policy"
                ));
            };
            Ok(ToolApprovalPolicy::PreApproved(tools))
        }
        "mixed" => {
            let tools = if let Some(tools_str) = pre_approved_tools {
                tools_str.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                std::collections::HashSet::new()
            };
            Ok(ToolApprovalPolicy::Mixed {
                pre_approved: tools,
                ask_for_others: true,
            })
        }
        _ => Err(anyhow!(
            "Invalid tool policy: {}. Valid options: always_ask, pre_approved, mixed",
            policy_str
        )),
    }
}

fn parse_metadata(metadata_str: Option<&str>) -> Result<std::collections::HashMap<String, String>> {
    let mut metadata = std::collections::HashMap::new();

    if let Some(meta_str) = metadata_str {
        for pair in meta_str.split(',') {
            let parts: Vec<&str> = pair.split('=').collect();
            if parts.len() != 2 {
                return Err(anyhow!(
                    "Invalid metadata format. Expected key=value pairs separated by commas"
                ));
            }
            metadata.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
    }

    Ok(metadata)
}
