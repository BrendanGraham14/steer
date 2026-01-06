use crate::tui::commands::{CoreCommandType, TuiCommandType};
use crate::tui::custom_commands::{CustomCommand, load_custom_commands};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub scope: CommandScope,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandScope {
    /// Commands handled only by the TUI
    TuiOnly,
    /// Commands that require server/core support
    Core,
    /// Custom user-defined commands
    Custom(CustomCommand),
}

/// Registry of all available slash commands
pub struct CommandRegistry {
    commands: HashMap<String, CommandInfo>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut commands = HashMap::new();

        // Add TUI commands from the enum
        for cmd_type in TuiCommandType::iter() {
            commands.insert(
                cmd_type.command_name(),
                CommandInfo {
                    name: cmd_type.command_name(),
                    description: cmd_type.description().to_string(),
                    usage: cmd_type.usage(),
                    scope: CommandScope::TuiOnly,
                },
            );
        }

        // Add Core commands from the enum
        for cmd_type in CoreCommandType::iter() {
            commands.insert(
                cmd_type.command_name(),
                CommandInfo {
                    name: cmd_type.command_name(),
                    description: cmd_type.description().to_string(),
                    usage: cmd_type.usage(),
                    scope: CommandScope::Core,
                },
            );
        }

        // Load custom commands
        match load_custom_commands() {
            Ok(custom_commands) => {
                debug!("Loading {} custom commands", custom_commands.len());
                for custom_cmd in custom_commands {
                    let display_name = custom_cmd.name().to_string();
                    debug!(
                        "Registering custom command: {} - {}",
                        display_name.clone(),
                        custom_cmd.description()
                    );
                    commands.insert(
                        display_name.clone(),
                        CommandInfo {
                            name: display_name.clone(),
                            description: custom_cmd.description().to_string(),
                            usage: format!("/{display_name}"),
                            scope: CommandScope::Custom(custom_cmd),
                        },
                    );
                }
            }
            Err(e) => {
                warn!("Failed to load custom commands: {}", e);
            }
        }

        CommandRegistry { commands }
    }

    /// Get all commands as a vector sorted by name
    pub fn all_commands(&self) -> Vec<&CommandInfo> {
        let mut commands: Vec<_> = self.commands.values().collect();
        commands.sort_by_key(|cmd| cmd.name.clone());
        commands
    }

    /// Search for commands matching a query using fuzzy matching
    pub fn search(&self, query: &str) -> Vec<&CommandInfo> {
        if query.is_empty() {
            return self.all_commands();
        }

        let matcher = SkimMatcherV2::default();

        let mut scored_commands: Vec<(i64, &CommandInfo)> = self
            .commands
            .values()
            .filter_map(|cmd| {
                // Match against both command name and description
                let name_score = matcher.fuzzy_match(&cmd.name, query).unwrap_or(0);
                let desc_score = matcher.fuzzy_match(&cmd.description, query).unwrap_or(0);

                // Use the higher score between name and description
                let best_score = name_score.max(desc_score);

                if best_score > 0 {
                    // Boost score if query matches start of command name
                    let boosted_score = if cmd.name.starts_with(query) {
                        best_score + 1000
                    } else {
                        best_score
                    };
                    Some((boosted_score, cmd))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score (highest first)
        scored_commands.sort_by(|a, b| b.0.cmp(&a.0));

        scored_commands.into_iter().map(|(_, cmd)| cmd).collect()
    }

    /// Get a specific command by name
    pub fn get(&self, name: &str) -> Option<&CommandInfo> {
        self.commands.get(name)
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_search_exact_match() {
        let registry = CommandRegistry::new();
        let results = registry.search("model");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "model");
        assert_eq!(results[1].name, "editing-mode");
    }

    #[test]
    fn test_fuzzy_search_partial_match() {
        let registry = CommandRegistry::new();
        let results = registry.search("mod");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "model");
    }

    #[test]
    fn test_fuzzy_search_description_match() {
        let registry = CommandRegistry::new();
        let results = registry.search("change");
        assert!(!results.is_empty());
        // Should find commands with "change" in their description
        let names: Vec<&str> = results.iter().map(|cmd| cmd.name.as_str()).collect();
        // Both "model" and "theme" have "change" in their descriptions
        assert!(names.contains(&"model") || names.contains(&"theme"));
    }

    #[test]
    fn test_fuzzy_search_multiple_matches() {
        let registry = CommandRegistry::new();
        let results = registry.search("c");
        // Should find "compact" (and possibly others like "mcp")
        assert!(!results.is_empty());
        let names: Vec<&str> = results.iter().map(|cmd| cmd.name.as_str()).collect();
        assert!(names.contains(&"compact"));
    }

    #[test]
    fn test_fuzzy_search_no_match() {
        let registry = CommandRegistry::new();
        let results = registry.search("xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_search_empty_query() {
        let registry = CommandRegistry::new();
        let results = registry.search("");
        // Should return all commands
        assert_eq!(results.len(), registry.all_commands().len());
    }

    #[test]
    fn test_command_name_prefix_boost() {
        let registry = CommandRegistry::new();
        let results = registry.search("ne");
        // "new" should come before other matches because it starts with "ne"
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "new");
    }

    #[test]
    fn test_get_specific_command() {
        let registry = CommandRegistry::new();
        let cmd = registry.get("model");
        assert!(cmd.is_some());
        assert_eq!(cmd.unwrap().name, "model");
    }

    #[test]
    fn test_get_nonexistent_command() {
        let registry = CommandRegistry::new();
        let cmd = registry.get("nonexistent");
        assert!(cmd.is_none());
    }

    #[test]
    fn test_all_commands_sorted() {
        let registry = CommandRegistry::new();
        let commands = registry.all_commands();
        // Verify they're sorted alphabetically
        for i in 1..commands.len() {
            assert!(commands[i - 1].name.as_str() <= commands[i].name.as_str());
        }
    }
}
