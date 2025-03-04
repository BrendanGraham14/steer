use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use serde::{Serialize, Deserialize};

/// Information about the environment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub working_directory: PathBuf,
    pub is_git_repo: bool,
    pub platform: String,
    pub date: String,
    pub directory_structure: String,
    pub git_status: Option<String>,
    pub readme_content: Option<String>,
    pub claude_md_content: Option<String>,
}

impl EnvironmentInfo {
    /// Collect information about the environment
    pub fn collect() -> Result<Self> {
        let working_directory = std::env::current_dir()
            .context("Failed to get current directory")?;
        
        // Check if we're in a git repo
        let is_git_repo = Path::new(".git").exists() || Self::is_git_repo()?;
        
        // Get platform information
        let platform = Self::get_platform();
        
        // Get current date
        let date = Self::get_date();
        
        // Get directory structure
        let directory_structure = Self::get_directory_structure(&working_directory)?;
        
        // Get git status if in a repo
        let git_status = if is_git_repo {
            Some(Self::get_git_status()?)
        } else {
            None
        };
        
        // Check for README.md
        let readme_content = Self::read_file_if_exists("README.md");
        
        // Check for CLAUDE.md
        let claude_md_content = Self::read_file_if_exists("CLAUDE.md");
        
        Ok(Self {
            working_directory,
            is_git_repo,
            platform,
            date,
            directory_structure,
            git_status,
            readme_content,
            claude_md_content,
        })
    }
    
    /// Check if the current directory is in a git repo
    fn is_git_repo() -> Result<bool> {
        let output = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .output();
            
        match output {
            Ok(output) if output.status.success() => Ok(true),
            _ => Ok(false),
        }
    }
    
    /// Get the platform information
    fn get_platform() -> String {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        };
        
        os.to_string()
    }
    
    /// Get the current date
    fn get_date() -> String {
        use chrono::Local;
        Local::now().format("%Y-%m-%d").to_string()
    }
    
    /// Get the directory structure
    fn get_directory_structure(dir: &Path) -> Result<String> {
        // Use a simpler approach to get directory structure
        // We don't want to list all files, just a high-level overview
        let mut result = String::new();
        result.push_str(&format!("- {}\n", dir.display()));
        
        // First level of directories
        let entries = fs::read_dir(dir)
            .context("Failed to read directory")?;
            
        for entry in entries {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            
            // Skip hidden files and directories
            if file_name.starts_with('.') {
                continue;
            }
            
            if path.is_dir() {
                result.push_str(&format!("  - {}/\n", file_name));
                
                // Get second level directories (just names, not recursive)
                if let Ok(subentries) = fs::read_dir(&path) {
                    for subentry in subentries {
                        if let Ok(subentry) = subentry {
                            let subpath = subentry.path();
                            let subname = subpath.file_name().unwrap_or_default().to_string_lossy();
                            
                            // Skip hidden files
                            if subname.starts_with('.') {
                                continue;
                            }
                            
                            if subpath.is_dir() {
                                result.push_str(&format!("    - {}/\n", subname));
                            } else {
                                result.push_str(&format!("    - {}\n", subname));
                            }
                        }
                    }
                }
            } else {
                result.push_str(&format!("  - {}\n", file_name));
            }
        }
        
        Ok(result)
    }
    
    /// Get git status information
    fn get_git_status() -> Result<String> {
        let mut result = String::new();
        
        // Get current branch
        let branch_output = Command::new("git")
            .args(["branch", "--show-current"])
            .output()
            .context("Failed to run git branch")?;
            
        let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();
        result.push_str(&format!("Current branch: {}\n\n", branch));
        
        // Get status
        let status_output = Command::new("git")
            .args(["status", "--short"])
            .output()
            .context("Failed to run git status")?;
            
        let status = String::from_utf8_lossy(&status_output.stdout).trim().to_string();
        result.push_str("Status:\n");
        if status.is_empty() {
            result.push_str("Working tree clean\n");
        } else {
            result.push_str(&status);
            result.push('\n');
        }
        
        // Get recent commits
        let log_output = Command::new("git")
            .args(["log", "--oneline", "-n", "5"])
            .output()
            .context("Failed to run git log")?;
            
        let log = String::from_utf8_lossy(&log_output.stdout).trim().to_string();
        result.push_str("\nRecent commits:\n");
        result.push_str(&log);
        
        Ok(result)
    }
    
    /// Read a file if it exists
    fn read_file_if_exists(filename: &str) -> Option<String> {
        match fs::read_to_string(filename) {
            Ok(content) => Some(content),
            Err(_) => None,
        }
    }
    
    /// Format environment information as context for the model
    pub fn as_context(&self) -> String {
        let mut context = String::new();
        
        context.push_str("<context name=\"directoryStructure\">Below is a snapshot of this project's file structure at the start of the conversation. This snapshot will NOT update during the conversation.\n\n");
        context.push_str(&self.directory_structure);
        context.push_str("</context>\n");
        
        if let Some(git_status) = &self.git_status {
            context.push_str("<context name=\"gitStatus\">This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.\n");
            context.push_str(git_status);
            context.push_str("</context>\n");
        }
        
        if let Some(readme) = &self.readme_content {
            context.push_str("<context name=\"readme\">\n");
            context.push_str(readme);
            context.push_str("</context>\n");
        }
        
        context
    }
    
    /// Format environment information as env for the model
    pub fn as_env(&self) -> String {
        format!(
            "<env>\nWorking directory: {}\nIs directory a git repo: {}\nPlatform: {}\nToday's date: {}\nModel: claude-3-7-sonnet-20250219\n</env>",
            self.working_directory.display(),
            if self.is_git_repo { "Yes" } else { "No" },
            self.platform,
            self.date
        )
    }
}