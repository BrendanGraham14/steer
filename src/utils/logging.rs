use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Log levels for different types of messages
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warning => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// Logger instance that handles writing logs to a file
pub struct Logger {
    file: Option<Arc<Mutex<File>>>,
    level: LogLevel,
    enabled: bool,
}

static LOGGER: OnceLock<Arc<Mutex<Logger>>> = OnceLock::new();

impl Logger {
    /// Initialize the global logger instance
    pub fn init(log_file: Option<&Path>, level: LogLevel) -> io::Result<()> {
        let file = if let Some(path) = log_file {
            // Create the directory if it doesn't exist
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            // Open the log file (create if it doesn't exist, append if it does)
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            
            Some(Arc::new(Mutex::new(file)))
        } else {
            None
        };
        
        let logger = Logger {
            file,
            level,
            enabled: true,
        };
        
        // Set the global logger instance
        let _ = LOGGER.set(Arc::new(Mutex::new(logger)));
        
        Ok(())
    }
    
    /// Get the global logger instance
    pub fn get() -> Arc<Mutex<Logger>> {
        LOGGER.get().cloned().unwrap_or_else(|| {
            // Create a default logger if none exists
            let logger = Logger {
                file: None,
                level: LogLevel::Info,
                enabled: false,
            };
            Arc::new(Mutex::new(logger))
        })
    }
    
    /// Write a message to the log
    pub fn log(&self, level: LogLevel, module: &str, message: &str) -> io::Result<()> {
        // Skip if logging is disabled or if the message level is below the logger level
        if !self.enabled || level < self.level {
            return Ok(());
        }
        
        // Get timestamp for the log entry
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let now = chrono::Local::now();
        let time_str = now.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        
        // Format the log entry
        let entry = format!("[{}] [{}] [{}]: {}\n", time_str, level, module, message);
        
        if let Some(file_lock) = &self.file {
            if let Ok(mut file) = file_lock.lock() {
                // Write to the log file
                file.write_all(entry.as_bytes())?;
                file.flush()?;
            }
        }
        
        Ok(())
    }
    
    /// Enable or disable logging
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    
    /// Change the log level
    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }
}

/// Log a debug message
pub fn debug(module: &str, message: &str) {
    if let Ok(logger) = Logger::get().lock() {
        let _ = logger.log(LogLevel::Debug, module, message);
    }
}

/// Log an info message
pub fn info(module: &str, message: &str) {
    if let Ok(logger) = Logger::get().lock() {
        let _ = logger.log(LogLevel::Info, module, message);
    }
}

/// Log a warning message
pub fn warn(module: &str, message: &str) {
    if let Ok(logger) = Logger::get().lock() {
        let _ = logger.log(LogLevel::Warning, module, message);
    }
}

/// Log an error message
pub fn error(module: &str, message: &str) {
    if let Ok(logger) = Logger::get().lock() {
        let _ = logger.log(LogLevel::Error, module, message);
    }
}

/// Initialize logging system with default settings
pub fn init_logging() -> io::Result<()> {
    // Default log file in the user's home directory with timestamp
    let now = chrono::Local::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    
    let home = dirs::home_dir();
    let log_path = home.map(|h| h.join(".claude-code").join(format!("{}.log", timestamp)));
    
    Logger::init(log_path.as_deref(), LogLevel::Debug)
}