use ratatui::{
    style::Style,
    text::{Line, Span},
};
use textwrap;
use unicode_width::UnicodeWidthStr;

/// Truncate text to a maximum number of lines, adding an ellipsis if truncated
pub fn truncate_lines(text: &str, max_lines: usize) -> (Vec<&str>, bool) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        (lines, false)
    } else {
        (lines.into_iter().take(max_lines).collect(), true)
    }
}

/// Wrap lines to fit within a maximum width
pub fn wrap_lines<'a>(lines: impl Iterator<Item = &'a str>, width: usize) -> Vec<String> {
    lines
        .flat_map(|line| {
            textwrap::wrap(line, width)
                .into_iter()
                .map(|s| s.to_string())
        })
        .collect()
}

/// Create a single-line JSON preview with whitespace collapsed
pub fn json_preview(value: &serde_json::Value, max_len: usize) -> String {
    let mut preview = serde_json::to_string(value).unwrap_or_default();
    preview.retain(|c| !c.is_whitespace());

    if preview.len() > max_len {
        preview.truncate(max_len.saturating_sub(3));
        preview.push_str("...");
    }

    preview
}

/// Create a separator line
pub fn separator_line(width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width.min(40)), style))
}

/// Truncate a long string in the middle, preserving start & end, with ellipsis.
pub fn truncate_middle(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    if max_len <= 3 {
        return "...".chars().take(max_len).collect();
    }
    let keep = (max_len - 3) / 2;
    let start = &text[..keep];
    let end = &text[text.len() - keep..];
    format!("{}...{}", start, end)
}

/// Format file size in human-readable format
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Extract exit code from error message
pub fn extract_exit_code(error: &str) -> Option<&str> {
    error
        .lines()
        .find(|l| l.starts_with("Exit code:"))
        .and_then(|l| l.split(": ").nth(1))
}

/// Wrap a styled line while preserving span styles
/// This function breaks lines at word boundaries while maintaining the style of each span
pub fn style_wrap(line: Line<'_>, max_width: u16) -> Vec<Line<'static>> {
    let mut output_lines = Vec::new();
    let mut current_line_spans = Vec::new();
    let mut current_width = 0;

    for span in line.spans {
        let style = span.style;
        let content = span.content.as_ref();

        // Split content by whitespace but keep the whitespace attached to words
        let words = content.split_inclusive(' ');

        for word in words {
            let word_width = word.width();

            // Check if adding this word would exceed the line width
            if current_width > 0 && current_width + word_width > max_width as usize {
                // Push the current line and start a new one
                if !current_line_spans.is_empty() {
                    output_lines.push(Line::from(current_line_spans));
                    current_line_spans = Vec::new();
                    current_width = 0;
                }
            }

            // Add the word to the current line
            current_line_spans.push(Span::styled(word.to_string(), style));
            current_width += word_width;
        }
    }

    // Push any remaining spans as the last line
    if !current_line_spans.is_empty() {
        output_lines.push(Line::from(current_line_spans));
    }

    // If no lines were generated, return an empty line
    if output_lines.is_empty() {
        output_lines.push(Line::from(""));
    }

    output_lines
}
