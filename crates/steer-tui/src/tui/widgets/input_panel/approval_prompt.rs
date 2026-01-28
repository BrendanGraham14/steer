//! Tool approval prompt widget

use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, Widget};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use steer_tools::schema::ToolCall;

use crate::tui::theme::{Component, Theme};

/// Widget for displaying tool approval prompts
#[derive(Debug)]
pub struct ApprovalWidget<'a> {
    tool_call: &'a ToolCall,
    theme: &'a Theme,
}

impl<'a> ApprovalWidget<'a> {
    /// Create a new approval widget
    pub fn new(tool_call: &'a ToolCall, theme: &'a Theme) -> Self {
        Self { tool_call, theme }
    }

    /// Format the approval text for the tool call
    fn format_approval_text(&self, area_width: u16) -> Vec<Line<'static>> {
        let formatter = crate::tui::widgets::formatters::get_formatter(&self.tool_call.name);
        let preview_lines = formatter.approval(
            &self.tool_call.parameters,
            (area_width.saturating_sub(4)) as usize,
            self.theme,
        );

        let is_bash_command = self.tool_call.name == "bash";

        let mut approval_text = if is_bash_command {
            vec![
                Line::from(vec![
                    Span::styled("Tool ", Style::default()),
                    Span::styled(
                        self.tool_call.name.clone(),
                        self.theme.style(Component::ToolCallHeader),
                    ),
                    Span::styled(" wants to run this shell command", Style::default()),
                ]),
                Line::from(""),
            ]
        } else {
            vec![
                Line::from(vec![
                    Span::styled("Tool ", Style::default()),
                    Span::styled(
                        self.tool_call.name.clone(),
                        self.theme.style(Component::ToolCallHeader),
                    ),
                    Span::styled(" needs your approval", Style::default()),
                ]),
                Line::from(""),
            ]
        };

        approval_text.extend(preview_lines);
        approval_text
    }

    /// Get the keybind options for approval
    fn get_approval_keybinds(&self) -> Vec<(Span<'static>, Span<'static>)> {
        let is_bash_command = self.tool_call.name == "bash";

        if is_bash_command {
            vec![
                (
                    Span::styled("[Y]", self.theme.style(Component::ToolSuccess)),
                    Span::styled("Yes (once)", self.theme.style(Component::DimText)),
                ),
                (
                    Span::styled("[A]", self.theme.style(Component::ToolSuccess)),
                    Span::styled(
                        "Always (this command)",
                        self.theme.style(Component::DimText),
                    ),
                ),
                (
                    Span::styled("[L]", self.theme.style(Component::ToolSuccess)),
                    Span::styled(
                        "Always (all Bash commands)",
                        self.theme.style(Component::DimText),
                    ),
                ),
                (
                    Span::styled("[N]", self.theme.style(Component::ToolError)),
                    Span::styled("No", self.theme.style(Component::DimText)),
                ),
            ]
        } else {
            vec![
                (
                    Span::styled("[Y]", self.theme.style(Component::ToolSuccess)),
                    Span::styled("Yes (once)", self.theme.style(Component::DimText)),
                ),
                (
                    Span::styled("[A]", self.theme.style(Component::ToolSuccess)),
                    Span::styled("Always", self.theme.style(Component::DimText)),
                ),
                (
                    Span::styled("[N]", self.theme.style(Component::ToolError)),
                    Span::styled("No", self.theme.style(Component::DimText)),
                ),
            ]
        }
    }

    /// Format the title line with keybinds
    fn format_title(&self) -> Line<'static> {
        let approval_keybinds = self.get_approval_keybinds();
        let mut title_spans = vec![Span::raw(" Approval Required "), Span::raw("─ ")];

        for (i, (key, desc)) in approval_keybinds.iter().enumerate() {
            if i > 0 {
                title_spans.push(Span::styled(" │ ", self.theme.style(Component::DimText)));
            }
            title_spans.push(key.clone());
            title_spans.push(Span::raw(" "));
            title_spans.push(desc.clone());
        }
        title_spans.push(Span::raw(" "));

        Line::from(title_spans)
    }
}

impl Widget for ApprovalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let approval_text = self.format_approval_text(area.width);
        let title = self.format_title();

        let approval_block = Paragraph::new(approval_text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(self.theme.style(Component::InputPanelBorderApproval)),
        );

        approval_block.render(area, buf);
    }
}
