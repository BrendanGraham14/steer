//! Widget wrappers for tool formatters
//!
//! This module provides widget implementations that wrap the existing
//! formatters to render tool calls and results within bounded areas.

use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::ChatWidget;
use crate::tui::widgets::formatters;
use crate::tui::{theme::Theme, widgets::chat_widgets::chat_widget::HeightCache};
use conductor_tools::{ToolCall, ToolResult};
use ratatui::{buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

/// Widget wrapper for tool formatters
pub struct ToolWidget {
    tool_call: ToolCall,
    result: Option<ToolResult>,
    cache: HeightCache,
    rendered_compact_lines: Option<Vec<Line<'static>>>,
    rendered_detailed_lines: Option<Vec<Line<'static>>>,
}

impl ToolWidget {
    pub fn new(tool_call: ToolCall, result: Option<ToolResult>) -> Self {
        Self {
            tool_call,
            result,
            cache: HeightCache::new(),
            rendered_compact_lines: None,
            rendered_detailed_lines: None,
        }
    }

    fn render_lines(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> &Vec<Line<'static>> {
        if self.rendered_compact_lines.is_some()
            && self.rendered_detailed_lines.is_some()
            && self.cache.last_width == width
        {
            return match mode {
                ViewMode::Compact => self.rendered_compact_lines.as_ref().unwrap(),
                ViewMode::Detailed => self.rendered_detailed_lines.as_ref().unwrap(),
            };
        }

        let formatter = formatters::get_formatter(&self.tool_call.name);
        let wrap_width = width.saturating_sub(2) as usize;

        // Render both modes
        self.rendered_compact_lines =
            Some(formatter.compact(&self.tool_call.parameters, &self.result, wrap_width, theme));

        self.rendered_detailed_lines =
            Some(formatter.detailed(&self.tool_call.parameters, &self.result, wrap_width, theme));

        match mode {
            ViewMode::Compact => self.rendered_compact_lines.as_ref().unwrap(),
            ViewMode::Detailed => self.rendered_detailed_lines.as_ref().unwrap(),
        }
    }
}

impl ChatWidget for ToolWidget {
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize {
        // Check cache first
        if let Some(cached) = self.cache.get(mode, width) {
            return cached;
        }

        // Get the rendered lines
        let lines = self.render_lines(mode, width, theme);
        let height = lines.len();

        // Cache the result
        self.cache.set(mode, width, height);
        height
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, mode: ViewMode, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Get the rendered lines
        let lines = self.render_lines(mode, area.width, theme);

        let mut y = area.y;
        for line in lines.iter() {
            if y >= area.y + area.height {
                break;
            }
            buf.set_line(area.x, y, line, area.width);
            y += 1;
        }
    }

    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Get the rendered lines
        let lines = self.render_lines(mode, area.width, theme);
        let total_lines = lines.len();

        if first_line >= total_lines {
            return;
        }

        // Calculate which lines to render
        let last_line = (first_line + area.height as usize).min(total_lines);
        let visible_lines = &lines[first_line..last_line];

        let mut y = area.y;
        for line in visible_lines.iter() {
            if y >= area.y + area.height {
                break;
            }
            buf.set_line(area.x, y, line, area.width);
            y += 1;
        }
    }
}

/// Widget for pending tool calls (no result yet, with header)
pub struct PendingToolCallWidget {
    tool_call: ToolCall,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl PendingToolCallWidget {
    pub fn new(tool_call: ToolCall) -> Self {
        Self {
            tool_call,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }

    fn render_lines(&mut self, width: u16, theme: &Theme) -> &Vec<Line<'static>> {
        if self.rendered_lines.is_none() || self.cache.last_width != width {
            let formatter = formatters::get_formatter(&self.tool_call.name);
            let wrap_width = width.saturating_sub(2) as usize; // Account for indent inside RowWidget

            let mut lines = formatter.compact(&self.tool_call.parameters, &None, wrap_width, theme);

            // Build header line "<tool> ⋯ "
            let header = format!("{} ⋯ ", self.tool_call.name);
            if !lines.is_empty() {
                // prepend header into first line
                let mut first_spans = Vec::new();
                first_spans.push(ratatui::text::Span::styled(
                    header,
                    theme.style(crate::tui::theme::Component::ToolCallHeader),
                ));
                first_spans.extend(lines[0].spans.clone());
                lines[0] = Line::from(first_spans);
            } else {
                lines.push(Line::from(ratatui::text::Span::styled(
                    header,
                    theme.style(crate::tui::theme::Component::ToolCallHeader),
                )));
            }
            self.rendered_lines = Some(lines);
        }
        self.rendered_lines.as_ref().unwrap()
    }
}

impl ChatWidget for PendingToolCallWidget {
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize {
        // pending tool call ignores view_mode (always compact-like)
        if let Some(cached) = self.cache.get(mode, width) {
            return cached;
        }
        let lines_len = self.render_lines(width, theme).len();
        self.cache.set(mode, width, lines_len);
        lines_len
    }

    fn render(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        _mode: ViewMode,
        theme: &Theme,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let lines = self.render_lines(area.width, theme).clone();
        let bg_style = theme.style(crate::tui::theme::Component::ChatListBackground);
        let para = ratatui::widgets::Paragraph::new(lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(bg_style);
        para.render(area, buf);
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

        // Ensure lines are rendered
        let lines = self.render_lines(area.width, theme);
        if first_line >= lines.len() {
            return;
        }

        // Calculate the slice of lines to render
        let end_line = (first_line + area.height as usize).min(lines.len());
        let visible_lines = &lines[first_line..end_line];

        // Create a paragraph with only the visible lines
        let bg_style = theme.style(crate::tui::theme::Component::ChatListBackground);
        let paragraph = ratatui::widgets::Paragraph::new(visible_lines.to_vec())
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(bg_style);

        paragraph.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::tools::{DISPATCH_AGENT_TOOL_NAME, FETCH_TOOL_NAME};
    use conductor_tools::tools::{
        AST_GREP_TOOL_NAME, GLOB_TOOL_NAME, LS_TOOL_NAME, REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME,
        TODO_WRITE_TOOL_NAME,
    };
    use serde_json::json;

    #[test]
    fn test_tool_widget() {
        let theme = Theme::default();
        let tool_call = ToolCall {
            id: "test-id".to_string(),
            name: "edit".to_string(),
            parameters: json!({
                "file_path": "/tmp/test.txt",
                "old_string": "hello",
                "new_string": "world"
            }),
        };

        let mut widget = ToolWidget::new(tool_call, None);

        // Test height calculation
        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert!(height > 0);

        // Test cache
        let height2 = widget.height(ViewMode::Compact, 80, &theme);
        assert_eq!(height, height2);

        // Test different mode
        let detailed_height = widget.height(ViewMode::Detailed, 80, &theme);
        assert!(detailed_height >= height); // Detailed should be at least as tall
    }

    #[test]
    fn test_edit_widget() {
        let theme = Theme::default();
        let tool_call = ToolCall {
            id: "test-id".to_string(),
            name: "edit".to_string(),
            parameters: json!({
                "file_path": "/tmp/test.txt",
                "old_string": "",
                "new_string": "Hello, world!"
            }),
        };

        let result = Some(ToolResult::Edit(conductor_tools::result::EditResult {
            file_path: "/tmp/test.txt".to_string(),
            changes_made: 1,
            file_created: true,
            old_content: None,
            new_content: Some("Hello, world!".to_string()),
        }));

        let mut widget = ToolWidget::new(tool_call, result);

        // Test that it renders without panicking
        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert!(height > 0);
    }

    #[test]
    fn test_bash_widget() {
        let theme = Theme::default();
        let tool_call = ToolCall {
            id: "test-id".to_string(),
            name: "bash".to_string(),
            parameters: json!({
                "command": "echo 'Hello, world!'"
            }),
        };

        let result = Some(ToolResult::Bash(conductor_tools::result::BashResult {
            command: "echo 'Hello, world!'".to_string(),
            exit_code: 0,
            stdout: "Hello, world!\n".to_string(),
            stderr: "".to_string(),
        }));

        let mut widget = ToolWidget::new(tool_call, result);

        // Test both modes
        let compact_height = widget.height(ViewMode::Compact, 80, &theme);
        let detailed_height = widget.height(ViewMode::Detailed, 80, &theme);
        assert!(compact_height > 0);
        assert!(detailed_height > 0);
        assert!(detailed_height >= compact_height);
    }

    #[test]
    fn test_grep_widget() {
        let theme = Theme::default();
        let tool_call = ToolCall {
            id: "test-id".to_string(),
            name: "grep".to_string(),
            parameters: json!({
                "pattern": "test",
                "path": "."
            }),
        };

        let result = Some(ToolResult::Search(conductor_tools::result::SearchResult {
            matches: vec![
                conductor_tools::result::SearchMatch {
                    file_path: "file1.txt".to_string(),
                    line_number: 10,
                    line_content: "This is a test line".to_string(),
                    column_range: None,
                },
                conductor_tools::result::SearchMatch {
                    file_path: "file1.txt".to_string(),
                    line_number: 20,
                    line_content: "Another test".to_string(),
                    column_range: None,
                },
                conductor_tools::result::SearchMatch {
                    file_path: "file2.txt".to_string(),
                    line_number: 5,
                    line_content: "Test case".to_string(),
                    column_range: None,
                },
            ],
            total_files_searched: 2,
            search_completed: true,
        }));

        let mut widget = ToolWidget::new(tool_call.clone(), result);

        // Test height calculation
        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert!(height > 0);

        // Test with no results
        let mut empty_widget = ToolWidget::new(tool_call, None);
        let empty_height = empty_widget.height(ViewMode::Compact, 80, &theme);
        assert!(empty_height > 0);
    }

    #[test]
    fn test_view_widget_with_large_content() {
        let theme = Theme::default();
        let tool_call = ToolCall {
            id: "test-id".to_string(),
            name: "view".to_string(),
            parameters: json!({
                "file_path": "/tmp/large.txt"
            }),
        };

        // Simulate a large file
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("Line {i}: Lorem ipsum dolor sit amet\n"));
        }

        let result = Some(ToolResult::FileContent(
            conductor_tools::result::FileContentResult {
                file_path: "/tmp/large.txt".to_string(),
                content,
                line_count: 100,
                truncated: true,
            },
        ));

        let mut widget = ToolWidget::new(tool_call, result);

        // Test that height is reasonable even with large content
        let compact_height = widget.height(ViewMode::Compact, 80, &theme);
        assert!(compact_height > 0);
        assert!(compact_height < 200); // Should be truncated

        let detailed_height = widget.height(ViewMode::Detailed, 80, &theme);
        assert!(detailed_height > compact_height);
    }

    #[test]
    fn test_all_widget_types() {
        let theme = Theme::default();

        // Test that all widget types can be instantiated and used
        let widget_configs = vec![
            (LS_TOOL_NAME, json!({"path": "/tmp"})),
            (GLOB_TOOL_NAME, json!({"pattern": "*.rs"})),
            (
                REPLACE_TOOL_NAME,
                json!({"file_path": "test.txt", "old_string": "foo", "new_string": "bar"}),
            ),
            (TODO_READ_TOOL_NAME, json!({})),
            (TODO_WRITE_TOOL_NAME, json!({"todos": []})),
            (AST_GREP_TOOL_NAME, json!({"pattern": "$FUNC($ARGS)"})),
            (
                FETCH_TOOL_NAME,
                json!({"url": "https://example.com", "prompt": "test"}),
            ),
            (DISPATCH_AGENT_TOOL_NAME, json!({"prompt": "test task"})),
        ];

        for (tool_name, params) in widget_configs {
            let tool_call = ToolCall {
                id: format!("{tool_name}-id"),
                name: tool_name.to_string(),
                parameters: params,
            };

            // Create widget based on tool name
            let mut widget: Box<dyn ChatWidget> = match tool_name {
                LS_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                GLOB_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                REPLACE_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                TODO_READ_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                TODO_WRITE_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                AST_GREP_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                FETCH_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                DISPATCH_AGENT_TOOL_NAME => Box::new(ToolWidget::new(tool_call, None)),
                _ => panic!("Unknown tool: {tool_name}"),
            };

            // Test that height calculation works
            let height = widget.height(ViewMode::Compact, 80, &theme);
            assert!(height > 0, "Widget {tool_name} should have non-zero height");
        }
    }
}
