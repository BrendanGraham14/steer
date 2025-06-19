use crate::api::Model;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// An AI-powered agent and CLI tool that assists with software engineering tasks.
#[derive(Parser)]
#[command(version, about, long_about = None, author)]
pub struct Cli {
    /// Optional directory to work in
    #[arg(short, long)]
    pub directory: Option<std::path::PathBuf>,

    /// Model to use
    #[arg(short, long, value_enum, default_value_t = Model::ClaudeSonnet4_20250514)]
    pub model: Model,

    /// Connect to a remote gRPC server instead of running locally
    #[arg(long)]
    pub remote: Option<String>,

    /// Custom system prompt to use instead of the default
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Subcommands
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
pub enum Commands {
    /// Initialize a new config file
    Init {
        /// Force overwrite of existing config
        #[arg(short, long)]
        force: bool,
    },
    /// Run in headless one-shot mode
    Headless {
        /// Model to use
        #[arg(long)]
        model: Option<Model>,

        /// JSON file containing a Vec<Message> to use. If not provided, reads prompt from stdin.
        #[arg(long)]
        messages_json: Option<PathBuf>,

        /// Session ID to run in (if not provided, creates a new ephemeral session)
        #[arg(long)]
        session: Option<String>,

        /// Path to JSON file containing SessionToolConfig for new sessions
        #[arg(long)]
        tool_config: Option<PathBuf>,

        /// Custom system prompt to use instead of the default
        #[arg(long)]
        system_prompt: Option<String>,
    },
    /// Start the gRPC server for client/server mode
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "50051")]
        port: u16,

        /// Bind address
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
    },
    /// Session management commands
    Session {
        #[command(subcommand)]
        session_command: SessionCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum SessionCommands {
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
        /// Custom system prompt to use instead of the default
        #[arg(long)]
        system_prompt: Option<String>,
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
