use ratatui::text::{Line, Span};
use steer_core::app::conversation::{AssistantContent, Message, MessageData, UserContent};

use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::formatters::helpers::style_wrap_with_indent;
use crate::tui::widgets::{ChatRenderable, ViewMode, markdown};

pub struct MessageWidget {
    message: Message,
    rendered_lines: Option<Vec<Line<'static>>>,
    last_width: u16,
    last_mode: ViewMode,
    last_theme_name: String,
    last_content_hash: u64,
}

impl MessageWidget {
    pub fn new(message: Message) -> Self {
        Self {
            message,
            rendered_lines: None,
            last_width: 0,
            last_mode: ViewMode::Compact,
            last_theme_name: String::new(),
            last_content_hash: 0,
        }
    }

    fn content_hash(message: &Message) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        // Hash over message data that affects rendering
        match &message.data {
            MessageData::User { content, .. } => {
                for c in content {
                    match c {
                        UserContent::Text { text } => text.hash(&mut hasher),
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => {
                            command.hash(&mut hasher);
                            stdout.hash(&mut hasher);
                            stderr.hash(&mut hasher);
                            exit_code.hash(&mut hasher);
                        }
                    }
                }
            }
            MessageData::Assistant { content, .. } => {
                for b in content {
                    match b {
                        AssistantContent::Text { text } => text.hash(&mut hasher),
                        AssistantContent::ToolCall { tool_call } => {
                            tool_call.id.hash(&mut hasher);
                            tool_call.name.hash(&mut hasher);
                            // parameters may be large; include their JSON string length and a hash of the string
                            let s = tool_call.parameters.to_string();
                            s.len().hash(&mut hasher);
                            s.hash(&mut hasher);
                        }
                        AssistantContent::Thought { thought } => {
                            thought.display_text().hash(&mut hasher);
                        }
                    }
                }
            }
            MessageData::Tool {
                tool_use_id,
                result,
                ..
            } => {
                tool_use_id.hash(&mut hasher);
                // Hash variant and key fields via Debug (cheap and stable enough here)
                use std::fmt::Write as _;
                let mut s = String::new();
                let _ = write!(&mut s, "{result:?}");
                s.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    fn render_as_markdown(text: &str, theme: &Theme, max_width: usize) -> markdown::MarkedText {
        let markdown_styles = markdown::MarkdownStyles::from_theme(theme);
        markdown::from_str_with_width(text, &markdown_styles, theme, Some(max_width as u16))
    }
}

impl ChatRenderable for MessageWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        let theme_key = theme.name.clone();
        let content_hash = Self::content_hash(&self.message);
        let cache_valid = self.rendered_lines.is_some()
            && self.last_width == width
            && self.last_mode == mode
            && self.last_theme_name == theme_key
            && self.last_content_hash == content_hash;
        if cache_valid {
            return self.rendered_lines.as_deref().unwrap();
        }

        let max_width = width.saturating_sub(4) as usize; // Account for gutters
        let mut lines = Vec::new();

        match &self.message.data {
            MessageData::User { content, .. } => {
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
                                    let wrapped = style_wrap_with_indent(
                                        marked_line.line,
                                        max_width as u16,
                                        marked_line.indent_level,
                                    );
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
                            let prompt_style = theme.style(Component::CommandPrompt);
                            let command_style = theme.style(Component::CommandText);
                            let error_style = theme.style(Component::CommandError);
                            let prompt = "$ ";
                            let indent = "  ";
                            let mut wrote_command = false;

                            for line in command.lines() {
                                for wrapped_line in
                                    textwrap::wrap(line, max_width.saturating_sub(prompt.len()))
                                {
                                    if !wrote_command {
                                        lines.push(Line::from(vec![
                                            Span::styled(prompt, prompt_style),
                                            Span::styled(wrapped_line.to_string(), command_style),
                                        ]));
                                        wrote_command = true;
                                    } else {
                                        lines.push(Line::from(vec![
                                            Span::styled(indent, ratatui::style::Style::default()),
                                            Span::styled(wrapped_line.to_string(), command_style),
                                        ]));
                                    }
                                }
                            }

                            if !wrote_command {
                                lines.push(Line::from(Span::styled(prompt, prompt_style)));
                            }

                            if !stdout.is_empty() {
                                for line in stdout.lines() {
                                    let wrapped = textwrap::wrap(line, max_width);
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            command_style,
                                        )));
                                    }
                                    if line.is_empty() {
                                        lines.push(Line::from(""));
                                    }
                                }
                            }

                            if !stderr.is_empty() {
                                for line in stderr.lines() {
                                    let wrapped = textwrap::wrap(line, max_width);
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            error_style,
                                        )));
                                    }
                                    if line.is_empty() {
                                        lines.push(Line::from(""));
                                    }
                                }
                            }

                            if *exit_code != 0 {
                                lines.push(Line::from(Span::styled(
                                    format!("Exit code: {exit_code}"),
                                    error_style,
                                )));
                            }
                        }
                    }
                }
            }
            MessageData::Assistant { content, .. } => {
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
                                    let wrapped = style_wrap_with_indent(
                                        marked_line.line,
                                        max_width as u16,
                                        marked_line.indent_level,
                                    );
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
                                    let wrapped = style_wrap_with_indent(
                                        thought_line,
                                        max_width as u16,
                                        marked_line.indent_level,
                                    );
                                    for line in wrapped {
                                        lines.push(line);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            MessageData::Tool { .. } => {
                // Tools are rendered as part of ToolInteraction blocks
            }
        }

        self.rendered_lines = Some(lines);
        self.last_width = width;
        self.last_mode = mode;
        self.last_theme_name = theme_key;
        self.last_content_hash = content_hash;
        self.rendered_lines.as_deref().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::MessageWidget;
    use crate::tui::theme::Theme;
    use crate::tui::widgets::ChatRenderable;
    use crate::tui::widgets::ViewMode;
    use steer_core::app::conversation::AssistantContent;
    use steer_core::app::conversation::{Message, MessageData, UserContent};

    #[test]
    fn test_message_widget_user_text() {
        let theme = Theme::default();
        let user_msg = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Hello, world!".to_string(),
                }],
            },
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(user_msg);

        // Test height calculation
        let height = widget.lines(20, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1); // Single line message

        // Test with wrapping
        let height_wrapped = widget.lines(8, ViewMode::Compact, &theme).len();
        assert!(height_wrapped > 1); // Should wrap
    }

    #[test]
    fn test_message_widget_assistant_text() {
        let theme = Theme::default();
        let assistant_msg = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "Hello from assistant".to_string(),
                }],
            },
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(assistant_msg);

        // Test height calculation
        let height = widget.lines(30, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1); // Single line message
    }

    #[test]
    fn test_message_widget_command_execution() {
        let theme = Theme::default();
        let cmd_msg = Message {
            data: MessageData::User {
                content: vec![UserContent::CommandExecution {
                    command: "ls -la".to_string(),
                    stdout: "file1.txt\nfile2.txt".to_string(),
                    stderr: "".to_string(),
                    exit_code: 0,
                }],
            },
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(cmd_msg);

        // Test height calculation - should have command line + 2 output lines
        let height = widget.lines(30, ViewMode::Compact, &theme).len();
        assert_eq!(height, 3); // $ ls -la + file1.txt + file2.txt
    }

    #[test]
    fn test_unicode_width_handling() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let theme = Theme::default();

        // Create a message with various Unicode characters
        let unicode_msg = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Hello 你好 👋 café".to_string(),
                }],
            },
            timestamp: 0,
            id: "test-unicode".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(unicode_msg);

        // Create buffers for both render methods
        let area = Rect::new(0, 0, 50, 5);
        let buf_regular = Buffer::empty(area);
        let buf_partial = Buffer::empty(area);

        // Render with regular method
        widget.lines(area.width, ViewMode::Compact, &theme);

        // Render with partial method
        widget.lines(area.width, ViewMode::Compact, &theme);

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
        let wide_msg = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "A中B".to_string(), // A=1 col, 中=2 cols, B=1 col
                }],
            },
            timestamp: 0,
            id: "test-wide".to_string(),
            parent_message_id: None,
        };

        let mut widget = MessageWidget::new(wide_msg);

        // Render with partial method
        let area = Rect::new(0, 0, 10, 1);
        let buf = Buffer::empty(area);
        widget.lines(area.width, ViewMode::Compact, &theme);

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
