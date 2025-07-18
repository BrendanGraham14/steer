use conductor_core::app::conversation::{AppCommandType, AssistantContent};
use conductor_core::app::conversation::{Message, UserContent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use tracing::debug;

use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::formatters::helpers::style_wrap;
use crate::tui::widgets::{ChatWidget, HeightCache, ViewMode, markdown};

pub struct MessageWidget {
    message: Message,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl MessageWidget {
    pub fn new(message: Message) -> Self {
        Self {
            message,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }

    fn render_lines(&mut self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        if self.rendered_lines.is_some() && self.cache.last_width == width {
            return self.rendered_lines.as_ref().unwrap().clone();
        }

        let max_width = width.saturating_sub(4) as usize; // Account for gutters
        let mut lines = Vec::new();

        match &self.message {
            Message::User { content, .. } => {
                for user_content in content {
                    match user_content {
                        UserContent::Text { text } => {
                            let marked_text = Self::render_as_markdown(text, theme, max_width);
                            for marked_line in marked_text.lines {
                                if marked_line.no_wrap {
                                    // Don't wrap code block lines
                                    lines.push(marked_line.line);
                                } else {
                                    // Wrap normal lines
                                    let wrapped = style_wrap(marked_line.line, max_width as u16);
                                    for line in wrapped {
                                        lines.push(line);
                                    }
                                }
                            }
                        }
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => {
                            // Format command execution
                            let cmd_style = theme.style(Component::CommandPrompt);
                            lines.push(Line::from(Span::styled(format!("$ {command}"), cmd_style)));

                            if !stdout.is_empty() {
                                let output_style = theme.style(Component::UserMessage);
                                for line in stdout.lines() {
                                    let wrapped = textwrap::wrap(line, max_width);
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            output_style,
                                        )));
                                    }
                                }
                            }

                            if !stderr.is_empty() {
                                let error_style = theme.style(Component::ErrorText);
                                for line in stderr.lines() {
                                    let wrapped = textwrap::wrap(line, max_width);
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            error_style,
                                        )));
                                    }
                                }
                            }

                            if *exit_code != 0 {
                                lines.push(Line::from(Span::styled(
                                    format!("Exit code: {exit_code}"),
                                    theme.style(Component::DimText),
                                )));
                            }
                        }
                        UserContent::AppCommand { command, response } => {
                            // Format app command
                            let cmd_style = theme.style(Component::CommandPrompt);
                            let cmd_text = match command {
                                AppCommandType::Model { target } => {
                                    if let Some(model) = target {
                                        format!("/model {model}")
                                    } else {
                                        "/model".to_string()
                                    }
                                }
                                AppCommandType::Compact => "/compact".to_string(),
                                AppCommandType::Clear => "/clear".to_string(),
                            };
                            lines.push(Line::from(Span::styled(cmd_text, cmd_style)));

                            if let Some(resp) = response {
                                let resp_text = match resp {
                                    conductor_core::app::conversation::CommandResponse::Text(text) => text.clone(),
                                    conductor_core::app::conversation::CommandResponse::Compact(result) => {
                                        match result {
                                            conductor_core::app::conversation::CompactResult::Success(summary) => summary.clone(),
                                            conductor_core::app::conversation::CompactResult::Cancelled => "Compact cancelled.".to_string(),
                                            conductor_core::app::conversation::CompactResult::InsufficientMessages => "Not enough messages to compact.".to_string(),
                                        }
                                    }
                                };

                                let resp_style = theme.style(Component::CommandText);
                                for line in resp_text.lines() {
                                    let wrapped = textwrap::wrap(line, max_width);
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            resp_style,
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Message::Assistant { content, .. } => {
                for block in content {
                    match block {
                        AssistantContent::Text { text } => {
                            if text.trim().is_empty() {
                                continue;
                            }

                            let marked_text = Self::render_as_markdown(text, theme, max_width);
                            for marked_line in marked_text.lines {
                                if marked_line.no_wrap {
                                    // Don't wrap code block lines
                                    lines.push(marked_line.line);
                                } else {
                                    // Wrap normal lines
                                    let wrapped = style_wrap(marked_line.line, max_width as u16);
                                    for line in wrapped {
                                        lines.push(line);
                                    }
                                }
                            }
                        }
                        AssistantContent::ToolCall { .. } => {
                            // Tool calls are rendered separately
                            continue;
                        }
                        AssistantContent::Thought { thought } => {
                            let thought_text = thought.display_text();
                            let thought_style = theme.style(Component::ThoughtText);

                            // Parse markdown for the thought
                            let markdown_styles = markdown::MarkdownStyles::from_theme(theme);
                            let markdown_text = markdown::from_str_with_width(
                                &thought_text,
                                &markdown_styles,
                                theme,
                                Some(max_width as u16),
                            );

                            // Process each line with thought styling
                            for marked_line in markdown_text.lines {
                                let mut styled_spans = Vec::new();

                                // Apply thought style to all spans
                                for span in marked_line.line.spans {
                                    styled_spans.push(Span::styled(
                                        span.content.into_owned(),
                                        thought_style,
                                    ));
                                }

                                let thought_line = Line::from(styled_spans);

                                if marked_line.no_wrap {
                                    lines.push(thought_line);
                                } else {
                                    let wrapped = style_wrap(thought_line, max_width as u16);
                                    for line in wrapped {
                                        lines.push(line);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Message::Tool { .. } => {
                // Tools are rendered as part of ToolInteraction blocks
            }
        }

        self.rendered_lines = Some(lines);
        self.rendered_lines.as_ref().unwrap().clone()
    }

    /// Create a paragraph with theme background applied
    fn create_themed_paragraph(
        &self,
        lines: Vec<Line<'static>>,
        theme: &Theme,
    ) -> Paragraph<'static> {
        let bg_style = theme.style(Component::ChatListBackground);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(bg_style)
    }

    fn render_as_markdown(text: &str, theme: &Theme, max_width: usize) -> markdown::MarkedText {
        let markdown_styles = markdown::MarkdownStyles::from_theme(theme);

        markdown::from_str_with_width(text, &markdown_styles, theme, Some(max_width as u16))
    }
}

impl ChatWidget for MessageWidget {
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize {
        if let Some(cached) = self.cache.get(mode, width) {
            return cached;
        }

        let height = self.render_lines(width, theme).len();
        self.cache.set(mode, width, height);
        height
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _mode: ViewMode, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            debug!("render: area is empty");
            return;
        }

        let lines = self.render_lines(area.width, theme);
        let paragraph = self.create_themed_paragraph(lines, theme);
        paragraph.render(area, buf);
    }

    #[tracing::instrument(skip(self, area, buf, _mode, theme), fields(first_line))]
    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let lines = self.render_lines(area.width, theme);
        let total_lines = lines.len();

        if first_line >= total_lines {
            return;
        }

        let last_line = (first_line + area.height as usize).min(total_lines);
        let lines_to_render = lines[first_line..last_line].to_vec();

        let paragraph = self.create_themed_paragraph(lines_to_render, theme);
        paragraph.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::MessageWidget;
    use crate::tui::theme::Theme;
    use crate::tui::widgets::ChatWidget;
    use crate::tui::widgets::ViewMode;
    use conductor_core::app::conversation::AssistantContent;
    use conductor_core::app::conversation::{Message, UserContent};

    #[test]
    fn test_message_widget_user_text() {
        let theme = Theme::default();
        let user_msg = Message::User {
            content: vec![UserContent::Text {
                text: "Hello, world!".to_string(),
            }],
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(user_msg);

        // Test height calculation
        let height = widget.height(ViewMode::Compact, 20, &theme);
        assert_eq!(height, 1); // Single line message

        // Test with wrapping
        let height_wrapped = widget.height(ViewMode::Compact, 8, &theme);
        assert!(height_wrapped > 1); // Should wrap
    }

    #[test]
    fn test_message_widget_assistant_text() {
        let theme = Theme::default();
        let assistant_msg = Message::Assistant {
            content: vec![AssistantContent::Text {
                text: "Hello from assistant".to_string(),
            }],
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(assistant_msg);

        // Test height calculation
        let height = widget.height(ViewMode::Compact, 30, &theme);
        assert_eq!(height, 1); // Single line message
    }

    #[test]
    fn test_message_widget_command_execution() {
        let theme = Theme::default();
        let cmd_msg = Message::User {
            content: vec![UserContent::CommandExecution {
                command: "ls -la".to_string(),
                stdout: "file1.txt\nfile2.txt".to_string(),
                stderr: "".to_string(),
                exit_code: 0,
            }],
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(cmd_msg);

        // Test height calculation - should have command line + 2 output lines
        let height = widget.height(ViewMode::Compact, 30, &theme);
        assert_eq!(height, 3); // $ ls -la + file1.txt + file2.txt
    }

    #[test]
    fn test_unicode_width_handling() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let theme = Theme::default();

        // Create a message with various Unicode characters
        let unicode_msg = Message::User {
            content: vec![UserContent::Text {
                text: "Hello 你好 👋 café".to_string(),
            }],
            timestamp: 0,
            id: "test-unicode".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(unicode_msg);

        // Create buffers for both render methods
        let area = Rect::new(0, 0, 50, 5);
        let mut buf_regular = Buffer::empty(area);
        let mut buf_partial = Buffer::empty(area);

        // Render with regular method
        widget.render(area, &mut buf_regular, ViewMode::Compact, &theme);

        // Render with partial method
        widget.render_partial(area, &mut buf_partial, ViewMode::Compact, &theme, 0);

        // Compare the buffers - they should be identical
        for y in 0..area.height {
            for x in 0..area.width {
                let regular_cell = buf_regular.cell((x, y)).unwrap();
                let partial_cell = buf_partial.cell((x, y)).unwrap();

                assert_eq!(
                    regular_cell.symbol(),
                    partial_cell.symbol(),
                    "Mismatch at ({}, {}): regular='{}' partial='{}'",
                    x,
                    y,
                    regular_cell.symbol(),
                    partial_cell.symbol()
                );
            }
        }
    }

    #[test]
    fn test_wide_character_positioning() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let theme = Theme::default();

        // Create a message with wide characters (CJK takes 2 columns each)
        let wide_msg = Message::User {
            content: vec![UserContent::Text {
                text: "A中B".to_string(), // A=1 col, 中=2 cols, B=1 col
            }],
            timestamp: 0,
            id: "test-wide".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(wide_msg);

        // Render with partial method
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        widget.render_partial(area, &mut buf, ViewMode::Compact, &theme, 0);

        // With correct Unicode handling:
        // Position 0: 'A' (1 column)
        // Position 1-2: '中' (2 columns)
        // Position 3: 'B' (1 column)

        // With the bug (incrementing by 1):
        // Position 0: 'A'
        // Position 1: '中' (but should occupy 2 columns)
        // Position 2: 'B' (overlapping with second half of '中')

        // This test will fail with current implementation
        // because 'B' will be at position 2 instead of position 3

        // Check that B is not at position 2 (would indicate the bug)
        let cell_at_2 = buf.cell((2, 0)).unwrap();
        assert_ne!(
            cell_at_2.symbol(),
            "B",
            "Character 'B' incorrectly positioned due to Unicode width bug"
        );
    }
}
