pub mod default;
pub mod gemini;
pub mod o3;

// Re-export the prompt functions for convenience
pub use default::default_system_prompt;
pub use gemini::gemini_system_prompt;
pub use o3::o3_system_prompt;
