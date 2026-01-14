//! Mode title widget for displaying input mode information and keybinds

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::InputMode;
use crate::tui::get_spinner_char;
use crate::tui::theme::{Component, Theme};

/// Widget for displaying the input mode title with keybinds
#[derive(Debug)]
pub struct ModeTitleWidget<'a> {
    mode: InputMode,
    is_processing: bool,
    spinner_state: usize,
    is_editing: bool,
    editing_preview: Option<&'a str>,
    theme: &'a Theme,
    has_content: bool,
}

impl<'a> ModeTitleWidget<'a> {
    /// Create a new mode title widget
    pub fn new(
        mode: InputMode,
        is_processing: bool,
        spinner_state: usize,
        is_editing: bool,
        editing_preview: Option<&'a str>,
        theme: &'a Theme,
        has_content: bool,
    ) -> Self {
        Self {
            mode,
            is_processing,
            spinner_state,
            is_editing,
            editing_preview,
            theme,
            has_content,
        }
    }

    /// Render the mode title as a Line
    pub fn render(&self) -> Line<'static> {
        let mut spans = vec![];

        // Add spinner if processing
        if self.is_processing {
            spans.push(Span::styled(
                format!(" {}", get_spinner_char(self.spinner_state)),
                self.theme.style(Component::ToolCall),
            ));
        }

        // Add mode title content
        spans.push(Span::raw(" "));

        if self.is_editing {
            spans.push(Span::styled(
                "Editing",
                self.theme.style(Component::InputPanelLabelEdit),
            ));
            if let Some(preview) = self.editing_preview {
                spans.push(Span::styled(": ", self.theme.style(Component::DimText)));
                spans.push(Span::styled(
                    preview.to_string(),
                    self.theme.style(Component::InputPanelLabel),
                ));
            }
            spans.push(Span::styled(" │ ", self.theme.style(Component::DimText)));
        }

        let formatted_mode = self.get_formatted_mode();
        if let Some(mode) = formatted_mode {
            spans.push(mode);
            spans.push(Span::styled(" │ ", self.theme.style(Component::DimText)));
        }

        // Add mode-specific keybinds
        let keybinds = self.get_mode_keybinds();
        spans.extend(format_keybinds(&keybinds, self.theme));

        spans.push(Span::raw(" "));
        Line::from(spans)
    }

    /// Get the formatted mode name with styling
    fn get_formatted_mode(&self) -> Option<Span<'static>> {
        let mode_name = match self.mode {
            InputMode::Simple => return None,
            InputMode::VimNormal => "NORMAL",
            InputMode::VimInsert => "INSERT",
            InputMode::BashCommand => "Bash",
            InputMode::AwaitingApproval => "Awaiting Approval",
            InputMode::ConfirmExit => "Confirm Exit",
            InputMode::EditMessageSelection => "Edit Selection",
            InputMode::FuzzyFinder => "Search",
            InputMode::Setup => "Setup",
        };

        let component = match self.mode {
            InputMode::ConfirmExit => Component::ErrorBold,
            InputMode::BashCommand => Component::CommandPrompt,
            InputMode::AwaitingApproval => Component::ErrorBold,
            InputMode::EditMessageSelection => Component::SelectionHighlight,
            InputMode::FuzzyFinder => Component::SelectionHighlight,
            _ => Component::ModelInfo,
        };

        Some(Span::styled(mode_name, self.theme.style(component)))
    }

    /// Get the keybinds for the current mode
    fn get_mode_keybinds(&self) -> Vec<(&'static str, &'static str)> {
        let mut keybinds = match self.mode {
            InputMode::Simple => {
                if self.has_content {
                    vec![("Enter", "send"), ("ESC ESC", "clear")]
                } else {
                    vec![
                        ("Enter", "send"),
                        ("ESC ESC", "edit previous"),
                        ("!", "bash"),
                        ("/", "command"),
                        ("@", "file"),
                    ]
                }
            }
            InputMode::VimNormal => {
                if self.has_content {
                    vec![("i", "insert"), ("ESC ESC", "clear"), ("hjkl", "move")]
                } else {
                    vec![
                        ("i", "insert"),
                        ("ESC ESC", "edit previous"),
                        ("!", "bash"),
                        ("/", "command"),
                    ]
                }
            }
            InputMode::VimInsert => {
                vec![("Esc", "normal"), ("ESC ESC", "clear"), ("Enter", "send")]
            }
            InputMode::BashCommand => {
                vec![("Enter", "execute"), ("Esc", "cancel")]
            }
            InputMode::AwaitingApproval => {
                // No keybinds for this mode
                vec![]
            }
            InputMode::ConfirmExit => {
                vec![("y/Y", "confirm"), ("any other key", "cancel")]
            }
            InputMode::EditMessageSelection => {
                vec![("↑↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")]
            }
            InputMode::FuzzyFinder => {
                vec![("↑↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")]
            }
            InputMode::Setup => {
                // No keybinds shown during setup mode
                vec![]
            }
        };

        if self.is_editing {
            keybinds.insert(0, ("Ctrl+E", "cancel edit"));
        }

        keybinds
    }
}

/// Helper function to format keybind hints with consistent styling
fn format_keybind(key: &str, description: &str, theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!("[{key}]"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {description}"), theme.style(Component::DimText)),
    ]
}

/// Format a list of keybinds with separators
fn format_keybinds(keybinds: &[(&str, &str)], theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (key, description)) in keybinds.iter().enumerate() {
        spans.extend(format_keybind(key, description, theme));
        if i < keybinds.len() - 1 {
            spans.push(Span::styled(" │ ", theme.style(Component::DimText)));
        }
    }
    spans
}
