//! Widget wrappers for tool formatters
//!
//! This module provides widget implementations that wrap the existing
//! formatters to render tool calls and results within bounded areas.

use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::ChatRenderable;
use crate::tui::widgets::formatters;
use crate::tui::{theme::Theme, widgets::chat_widgets::chat_widget::HeightCache};
use ratatui::text::Line;
use steer_tools::{ToolCall, ToolResult};

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
}

impl ChatRenderable for ToolWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        if self.rendered_compact_lines.is_some()
            && self.rendered_detailed_lines.is_some()
            && self.cache.last_width == width
        {
            return match mode {
                ViewMode::Compact => self.rendered_compact_lines.as_deref().unwrap_or(&[]),
                ViewMode::Detailed => self.rendered_detailed_lines.as_deref().unwrap_or(&[]),
            };
        }

        let formatter = formatters::get_formatter(&self.tool_call.name);
        let wrap_width = width.saturating_sub(2) as usize;

        // Render both modes
        let mut compact_lines =
            formatter.compact(&self.tool_call.parameters, &self.result, wrap_width, theme);
        let mut detailed_lines =
            formatter.detailed(&self.tool_call.parameters, &self.result, wrap_width, theme);

        // Add tool name header to the first line
        let header = format!("{} ", self.tool_call.name);
        if compact_lines.is_empty() {
            compact_lines.push(Line::from(ratatui::text::Span::styled(
                header.clone(),
                theme.style(crate::tui::theme::Component::ToolCallHeader),
            )));
        } else {
            // Prepend header to first line
            let mut first_spans = Vec::new();
            first_spans.push(ratatui::text::Span::styled(
                header.clone(),
                theme.style(crate::tui::theme::Component::ToolCallHeader),
            ));
            first_spans.extend(compact_lines[0].spans.clone());
            compact_lines[0] = Line::from(first_spans);
        }

        // Do the same for detailed view
        if detailed_lines.is_empty() {
            detailed_lines.push(Line::from(ratatui::text::Span::styled(
                header,
                theme.style(crate::tui::theme::Component::ToolCallHeader),
            )));
        } else {
            let mut first_spans = Vec::new();
            first_spans.push(ratatui::text::Span::styled(
                header,
                theme.style(crate::tui::theme::Component::ToolCallHeader),
            ));
            first_spans.extend(detailed_lines[0].spans.clone());
            detailed_lines[0] = Line::from(first_spans);
        }

        self.rendered_compact_lines = Some(compact_lines);
        self.rendered_detailed_lines = Some(detailed_lines);

        match mode {
            ViewMode::Compact => self.rendered_compact_lines.as_deref().unwrap_or(&[]),
            ViewMode::Detailed => self.rendered_detailed_lines.as_deref().unwrap_or(&[]),
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
}

impl ChatRenderable for PendingToolCallWidget {
    fn lines(&mut self, width: u16, _mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        if self.rendered_lines.is_none() || self.cache.last_width != width {
            let formatter = formatters::get_formatter(&self.tool_call.name);
            let wrap_width = width.saturating_sub(2) as usize; // Account for indent inside RowWidget

            let mut lines = formatter.compact(&self.tool_call.parameters, &None, wrap_width, theme);

            // Build header line "<tool> ⋯ "
            let header = format!("{} ⋯ ", self.tool_call.name);
            if lines.is_empty() {
                lines.push(Line::from(ratatui::text::Span::styled(
                    header,
                    theme.style(crate::tui::theme::Component::ToolCallHeader),
                )));
            } else {
                // prepend header into first line
                let mut first_spans = Vec::new();
                first_spans.push(ratatui::text::Span::styled(
                    header,
                    theme.style(crate::tui::theme::Component::ToolCallHeader),
                ));
                first_spans.extend(lines[0].spans.clone());
                lines[0] = Line::from(first_spans);
            }
            self.rendered_lines = Some(lines);
        }
        self.rendered_lines.as_deref().unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use steer_tools::tools::{
        AST_GREP_TOOL_NAME, DISPATCH_AGENT_TOOL_NAME, FETCH_TOOL_NAME, GLOB_TOOL_NAME,
        LS_TOOL_NAME, REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME,
    };

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
        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert!(height > 0);

        // Test cache
        let height2 = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, height2);

        // Test different mode
        let detailed_height = widget.lines(80, ViewMode::Detailed, &theme).len();
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

        let result = Some(ToolResult::Edit(steer_tools::result::EditResult {
            file_path: "/tmp/test.txt".to_string(),
            changes_made: 1,
            file_created: true,
            old_content: None,
            new_content: Some("Hello, world!".to_string()),
        }));

        let mut widget = ToolWidget::new(tool_call, result);

        // Test that it renders without panicking
        let height = widget.lines(80, ViewMode::Compact, &theme).len();
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

        let result = Some(ToolResult::Bash(steer_tools::result::BashResult {
            command: "echo 'Hello, world!'".to_string(),
            exit_code: 0,
            stdout: "Hello, world!\n".to_string(),
            stderr: String::new(),
            timed_out: false,
        }));

        let mut widget = ToolWidget::new(tool_call, result);

        // Test both modes
        let compact_height = widget.lines(80, ViewMode::Compact, &theme).len();
        let detailed_height = widget.lines(80, ViewMode::Detailed, &theme).len();
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

        let result = Some(ToolResult::Search(steer_tools::result::SearchResult {
            matches: vec![
                steer_tools::result::SearchMatch {
                    file_path: "file1.txt".to_string(),
                    line_number: 10,
                    line_content: "This is a test line".to_string(),
                    column_range: None,
                },
                steer_tools::result::SearchMatch {
                    file_path: "file1.txt".to_string(),
                    line_number: 20,
                    line_content: "Another test".to_string(),
                    column_range: None,
                },
                steer_tools::result::SearchMatch {
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
        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert!(height > 0);

        // Test with no results
        let mut empty_widget = ToolWidget::new(tool_call, None);
        let empty_height = empty_widget.lines(80, ViewMode::Compact, &theme).len();
        assert!(empty_height > 0);
    }

    #[test]
    fn test_view_widget_with_large_content() {
        let theme = Theme::default();
        let tool_call = ToolCall {
            id: "test-id".to_string(),
            name: steer_tools::tools::VIEW_TOOL_NAME.to_string(),
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
            steer_tools::result::FileContentResult {
                file_path: "/tmp/large.txt".to_string(),
                content,
                line_count: 100,
                truncated: true,
            },
        ));

        let mut widget = ToolWidget::new(tool_call, result);

        // View detailed mode should match compact mode and never show file content.
        let compact_height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert!(compact_height > 0);

        let detailed_height = widget.lines(80, ViewMode::Detailed, &theme).len();
        assert_eq!(detailed_height, compact_height);
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
            let mut widget: Box<dyn ChatRenderable> = match tool_name {
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
            let height = widget.lines(80, ViewMode::Compact, &theme).len();
            assert!(height > 0, "Widget {tool_name} should have non-zero height");
        }
    }
}
