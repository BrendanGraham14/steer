pub mod error;
pub mod notifications;
pub mod tui;
pub mod utils;

// Expose the main TUI entry point
pub use tui::Tui;

// Expose the run functions
pub use tui::{run_tui, run_tui_auth_setup};
