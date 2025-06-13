use chrono::Local;
use dirs;
use std::io;
use tracing_appender::rolling::{self};
use tracing_subscriber::{
    EnvFilter,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

/// Initialize the tracing system with either stdout or file logging.
///
/// Configuration behavior:
/// - In normal operation: Logs to file in ~/.coder directory
/// - Logging level is controlled by the RUST_LOG environment variable
pub fn init_tracing() -> io::Result<()> {
    // Default log file in the user's home directory with timestamp
    let now = Local::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");

    // Configure the filter based on RUST_LOG env var, with sensible defaults
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            // Default: info for all crates, debug for coder crate only, silence noisy crates
            EnvFilter::new("info,coder=debug,tui_markdown=warn")
        });

    if let Some(home_dir) = dirs::home_dir() {
        // Normal operation - log to file
        // Create the .coder directory if it doesn't exist
        let log_dir = home_dir.join(".coder");
        std::fs::create_dir_all(&log_dir)?;

        // Create the file appender directly (synchronous writing)
        let file_appender = rolling::never(log_dir.clone(), format!("{}.log", timestamp));

        let subscriber = tracing_subscriber::registry()
            .with(
                fmt::Layer::new()
                    .with_writer(file_appender)
                    .with_ansi(false)
                    .with_span_events(FmtSpan::CLOSE)
                    .with_file(true)
                    .with_line_number(true),
            )
            .with(filter);

        tracing::subscriber::set_global_default(subscriber)
            .expect("Failed to set global default subscriber");

        tracing::debug!(
            target: "coder::utils::tracing",
            path = %log_dir.join(format!("{}.log", timestamp)).display(),
            "Tracing initialized with file output. Filter configured via RUST_LOG env var."
        );
    } else {
        // Fallback to stdout if home directory not available
        let subscriber = tracing_subscriber::registry()
            .with(fmt::Layer::default().with_ansi(true).with_target(true))
            .with(filter);

        tracing::subscriber::set_global_default(subscriber)
            .expect("Failed to set global default subscriber");

        tracing::debug!(
            target: "coder::utils::tracing",
            "Tracing initialized with stdout output. Filter configured via RUST_LOG env var."
        );
    }

    Ok(())
}
