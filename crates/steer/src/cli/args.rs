use clap::{Parser, Subcommand};
use std::path::PathBuf;
use steer_core::api::Model;
use strum::IntoEnumIterator;

// Wrapper to implement clap::ValueEnum for Model
#[derive(Debug, Clone, Copy)]
pub struct ModelArg(pub Model);

impl From<ModelArg> for Model {
    fn from(arg: ModelArg) -> Self {
        arg.0
    }
}

impl clap::ValueEnum for ModelArg {
    fn value_variants<'a>() -> &'a [Self] {
        use once_cell::sync::Lazy;
        static VARIANTS: Lazy<Vec<ModelArg>> = Lazy::new(|| Model::iter().map(ModelArg).collect());
        &VARIANTS
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        let s: &'static str = self.0.into();
        let mut pv = clap::builder::PossibleValue::new(s);

        // Add all aliases from the Model enum
        for alias in self.0.aliases() {
            pv = pv.alias(alias);
        }

        Some(pv)
    }
}

/// An AI-powered agent and CLI tool that assists with software engineering tasks.
#[derive(Parser)]
#[command(version, about, long_about = None, author)]
pub struct Cli {
    /// Resume an existing session instead of starting a new one (local or remote modes)
    #[arg(long)]
    pub session: Option<String>,
    /// Optional directory to work in
    #[arg(short, long)]
    pub directory: Option<std::path::PathBuf>,

    /// Model to use
    #[arg(short, long, value_enum, default_value_t = ModelArg(Model::default()))]
    pub model: ModelArg,

    /// Connect to a remote gRPC server instead of running locally
    #[arg(long)]
    pub remote: Option<String>,

    /// Custom system prompt to use instead of the default
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Path to the session database file (defaults to ~/.steer/sessions.db)
    #[arg(long, env = "CONDUCTOR_SESSION_DB", hide = true)]
    pub session_db: Option<PathBuf>,

    /// Path to session configuration file (TOML format) for new sessions
    #[arg(long)]
    pub session_config: Option<PathBuf>,

    /// Theme to use for the TUI (defaults to "default")
    #[arg(long)]
    pub theme: Option<String>,

    /// Force the welcome/setup flow to run (for testing)
    #[arg(long, hide = true)]
    pub force_setup: bool,

    /// Subcommands
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
pub enum Commands {
    /// Launch the interactive terminal UI (default)
    Tui {
        /// Connect to a remote gRPC server (overrides global --remote)
        #[arg(long)]
        remote: Option<String>,
        /// Path to session configuration file (TOML format) for new sessions (overrides global)
        #[arg(long)]
        session_config: Option<PathBuf>,
        /// Theme to use for the TUI (overrides global)
        #[arg(long)]
        theme: Option<String>,
        /// Force the welcome/setup flow to run (for testing)
        #[arg(long, hide = true)]
        force_setup: bool,
    },
    /// Manage user preferences
    Preferences {
        #[command(subcommand)]
        action: PreferencesCommands,
    },
    /// Run in headless one-shot mode
    Headless {
        /// Model to use
        #[arg(long)]
        model: Option<ModelArg>,

        /// JSON file containing a Vec<Message> to use. If not provided, reads prompt from stdin.
        #[arg(long)]
        messages_json: Option<PathBuf>,

        /// Session ID to run in (if not provided, creates a new ephemeral session)
        #[arg(long)]
        session: Option<String>,

        /// Path to session configuration file (TOML format) for new sessions
        #[arg(long)]
        session_config: Option<PathBuf>,

        /// Custom system prompt to use instead of the default
        #[arg(long)]
        system_prompt: Option<String>,

        /// Connect to a remote gRPC server (overrides global --remote)
        #[arg(long)]
        remote: Option<String>,
    },
    /// Start the gRPC server
    Server {
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
    /// Show a notification (internal use only)
    #[clap(hide = true)]
    Notify {
        #[clap(long)]
        title: String,
        #[clap(long)]
        message: String,
        #[clap(long)]
        sound: Option<String>,
    },
}

#[derive(Subcommand, Clone)]
pub enum PreferencesCommands {
    /// Show current preferences
    Show,
    /// Edit preferences file in default editor
    Edit,
    /// Reset preferences to defaults
    Reset,
}

#[derive(Subcommand, Clone)]
pub enum SessionCommands {
    /// List all sessions
    List {
        /// Show only active sessions
        #[arg(long)]
        active: bool,
        /// Limit number of sessions to show
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// Create a new session
    Create {
        /// Path to session configuration file (TOML format)
        #[arg(long)]
        session_config: Option<PathBuf>,
        /// Session metadata (key=value pairs, comma-separated)
        #[arg(long)]
        metadata: Option<String>,
        /// Custom system prompt to use instead of the default
        #[arg(long)]
        system_prompt: Option<String>,
    },
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
