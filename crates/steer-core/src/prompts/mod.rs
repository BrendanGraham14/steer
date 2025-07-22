pub mod claude;
pub mod default;
pub mod gemini;
pub mod o3;

pub const MEMORY_FILE_NAME: &str = "CLAUDE.md";

// Re-export the prompt functions for convenience
pub use claude::claude_system_prompt;
pub use default::default_system_prompt;
pub use gemini::gemini_system_prompt;
pub use o3::o3_system_prompt;
