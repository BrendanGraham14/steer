use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Memory file manager for persisting information across sessions
pub struct MemoryManager {
    /// The path to the memory file
    file_path: Option<PathBuf>,
    /// The content of the memory file
    content: String,
}

impl MemoryManager {
    /// Create a new memory manager
    pub fn new(working_directory: &Path) -> Self {
        // Create the memory file path
        let file_path = working_directory.join("CLAUDE.md");
        
        // Check if the file exists
        let (file_path, content) = if file_path.exists() {
            // Read the file content
            let content = fs::read_to_string(&file_path).unwrap_or_default();
            (Some(file_path), content)
        } else {
            // File doesn't exist
            (None, String::new())
        };
        
        Self {
            file_path,
            content,
        }
    }
    
    /// Get the content of the memory file
    pub fn content(&self) -> &str {
        &self.content
    }
    
    /// Check if the memory file exists
    pub fn exists(&self) -> bool {
        self.file_path.is_some()
    }
    
    /// Add information to the memory file
    pub fn add_section(&mut self, section_name: &str, content: &str) -> Result<()> {
        // Create or update the section
        if self.content.is_empty() {
            // Create a new file with the section
            self.content = format!("# {}\n\n{}\n", section_name, content);
        } else {
            // Check if the section already exists
            let section_header = format!("# {}", section_name);
            if self.content.contains(&section_header) {
                // Section exists, update it
                // Split the content into sections
                let parts: Vec<&str> = self.content.split("# ").collect();
                
                // Find the section to update
                let mut new_content = String::new();
                for part in parts {
                    if part.is_empty() {
                        continue;
                    }
                    
                    if part.starts_with(section_name) {
                        // Update this section
                        new_content.push_str(&format!("# {}\n\n{}\n\n", section_name, content));
                    } else {
                        // Keep this section as is
                        new_content.push_str(&format!("# {}", part));
                    }
                }
                
                self.content = new_content;
            } else {
                // Section doesn't exist, add it
                self.content.push_str(&format!("\n# {}\n\n{}\n", section_name, content));
            }
        }
        
        // Save the file if it has a path
        if let Some(file_path) = &self.file_path {
            fs::write(file_path, &self.content)
                .context("Failed to write memory file")?;
        } else if !self.content.is_empty() {
            // Create the file now that we have content
            let file_path = Path::new(".").join("CLAUDE.md");
            fs::write(&file_path, &self.content)
                .context("Failed to write memory file")?;
            
            // Update the file path
            self.file_path = Some(file_path);
        }
        
        Ok(())
    }
    
    /// Get a specific section from the memory file
    pub fn get_section(&self, section_name: &str) -> Option<String> {
        if !self.exists() {
            return None;
        }
        
        // Look for the section
        let section_header = format!("# {}", section_name);
        if !self.content.contains(&section_header) {
            return None;
        }
        
        // Split the content into sections
        let parts: Vec<&str> = self.content.split("# ").collect();
        
        // Find the requested section
        for part in parts {
            if part.is_empty() {
                continue;
            }
            
            if part.starts_with(section_name) {
                // Extract the section content (removing the header)
                let lines: Vec<&str> = part.splitn(2, '\n').collect();
                if lines.len() > 1 {
                    return Some(lines[1].trim().to_string());
                } else {
                    return Some(String::new());
                }
            }
        }
        
        None
    }
}