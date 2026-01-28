use crate::tui::theme::{Component, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use similar::{Algorithm, ChangeTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    Unified,
    Split,
}

pub struct DiffWidget<'a> {
    old: &'a str,
    new: &'a str,
    mode: DiffMode,
    wrap_width: usize,
    theme: &'a Theme,
    context_radius: usize,
    max_lines: Option<usize>,
}

impl<'a> DiffWidget<'a> {
    pub fn new(old: &'a str, new: &'a str, theme: &'a Theme) -> Self {
        Self {
            old,
            new,
            mode: DiffMode::Unified,
            wrap_width: 80,
            theme,
            context_radius: 3,
            max_lines: None,
        }
    }

    pub fn with_mode(mut self, mode: DiffMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_wrap_width(mut self, width: usize) -> Self {
        self.wrap_width = width;
        self
    }

    pub fn with_context_radius(mut self, radius: usize) -> Self {
        self.context_radius = radius;
        self
    }

    pub fn with_max_lines(mut self, max: usize) -> Self {
        self.max_lines = Some(max);
        self
    }

    pub fn lines(&self) -> Vec<Line<'static>> {
        match self.mode {
            DiffMode::Unified => self.unified_diff(),
            DiffMode::Split => {
                // Fall back to unified if terminal is too narrow for split view
                // We need at least ~40 chars for split view to be useful (20 per side)
                if self.wrap_width < 40 {
                    self.unified_diff()
                } else {
                    self.split_diff()
                }
            }
        }
    }

    fn unified_diff(&self) -> Vec<Line<'static>> {
        let diff = TextDiff::configure()
            .algorithm(Algorithm::Myers)
            .diff_lines(self.old, self.new);

        let changes: Vec<_> = diff.iter_all_changes().collect();

        // First pass: determine which lines to show
        let mut show_line = vec![false; changes.len()];
        for (idx, change) in changes.iter().enumerate() {
            if change.tag() != ChangeTag::Equal {
                // Always show non-equal lines
                show_line[idx] = true;

                // Show context before
                let start = idx.saturating_sub(self.context_radius);
                for line in show_line.iter_mut().take(idx).skip(start) {
                    *line = true;
                }

                // Show context after
                let end = (idx + 1 + self.context_radius).min(changes.len());
                for line in show_line.iter_mut().take(end).skip(idx + 1) {
                    *line = true;
                }
            }
        }

        // Second pass: render lines with ellipsis for gaps
        let mut lines = Vec::new();
        let mut last_shown: Option<usize> = None;

        for (idx, (change, &should_show)) in changes.iter().zip(&show_line).enumerate() {
            if !should_show {
                continue;
            }

            // Add ellipsis if there's a gap
            match last_shown {
                None if idx > 0 => {
                    // Gap at beginning
                    lines.push(self.separator_line());
                }
                Some(last) if idx > last + 1 => {
                    // Gap in middle
                    lines.push(self.separator_line());
                }
                _ => {}
            }

            let (prefix, style) = match change.tag() {
                ChangeTag::Delete => ("-", self.theme.style(Component::CodeDeletion)),
                ChangeTag::Insert => ("+", self.theme.style(Component::CodeAddition)),
                ChangeTag::Equal => (" ", self.theme.style(Component::DimText)),
            };

            let content = change.value().trim_end();
            lines.extend(self.format_line(prefix, content, style));

            last_shown = Some(idx);

            // Check if we've hit the line limit
            if let Some(max) = self.max_lines {
                if lines.len() >= max {
                    let remaining = changes.len() - idx - 1;
                    if remaining > 0 {
                        lines.push(Line::from(Span::styled(
                            format!("... ({remaining} more lines)"),
                            self.theme
                                .style(Component::DimText)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                    break;
                }
            }
        }

        lines
    }

    fn split_diff(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Calculate split width (account for prefix and divider)
        let half_width = (self.wrap_width.saturating_sub(5)) / 2;

        let diff = TextDiff::configure()
            .algorithm(Algorithm::Myers)
            .diff_lines(self.old, self.new);

        // Group changes to properly handle replacements
        let changes: Vec<_> = diff.iter_all_changes().collect();
        let mut i = 0;

        while i < changes.len() {
            let change = changes[i];

            match change.tag() {
                ChangeTag::Equal => {
                    // Equal lines - show on both sides
                    let content = change.value().trim_end();
                    let left = self.truncate_or_pad(content, half_width);
                    let right = self.truncate_or_pad(content, half_width);

                    lines.push(Line::from(vec![
                        Span::styled(" ", self.theme.style(Component::DimText)),
                        Span::styled(left, self.theme.style(Component::DimText)),
                        Span::styled(" │ ", self.theme.style(Component::DimText)),
                        Span::styled(" ", self.theme.style(Component::DimText)),
                        Span::styled(right, self.theme.style(Component::DimText)),
                    ]));
                    i += 1;
                }
                ChangeTag::Delete => {
                    // Check if this is part of a replacement (delete followed by insert)
                    let mut deletes = vec![change];
                    let mut j = i + 1;

                    // Collect consecutive deletes
                    while j < changes.len() && changes[j].tag() == ChangeTag::Delete {
                        deletes.push(changes[j]);
                        j += 1;
                    }

                    // Check if followed by inserts
                    let mut inserts = Vec::new();
                    while j < changes.len() && changes[j].tag() == ChangeTag::Insert {
                        inserts.push(changes[j]);
                        j += 1;
                    }

                    if inserts.is_empty() {
                        // Pure deletion - show on left only
                        for del in deletes {
                            let content = del.value().trim_end();
                            let left = self.truncate_or_pad(content, half_width);
                            let right = " ".repeat(half_width);

                            lines.push(Line::from(vec![
                                Span::styled("-", self.theme.style(Component::CodeDeletion)),
                                Span::styled(left, self.theme.style(Component::CodeDeletion)),
                                Span::styled(" │ ", self.theme.style(Component::DimText)),
                                Span::styled(" ", self.theme.style(Component::DimText)),
                                Span::styled(right, self.theme.style(Component::DimText)),
                            ]));
                        }
                        i = j;
                    } else {
                        // This is a replacement - show side by side
                        let max_len = deletes.len().max(inserts.len());

                        for idx in 0..max_len {
                            let left_content = if idx < deletes.len() {
                                deletes[idx].value().trim_end()
                            } else {
                                ""
                            };
                            let right_content = if idx < inserts.len() {
                                inserts[idx].value().trim_end()
                            } else {
                                ""
                            };

                            let left = self.truncate_or_pad(left_content, half_width);
                            let right = self.truncate_or_pad(right_content, half_width);

                            // Determine prefixes based on whether there's content
                            let left_prefix = if left_content.is_empty() { " " } else { "-" };
                            let right_prefix = if right_content.is_empty() { " " } else { "+" };

                            lines.push(Line::from(vec![
                                Span::styled(
                                    left_prefix,
                                    self.theme.style(Component::CodeDeletion),
                                ),
                                Span::styled(
                                    left,
                                    if left_content.is_empty() {
                                        self.theme.style(Component::DimText)
                                    } else {
                                        self.theme.style(Component::CodeDeletion)
                                    },
                                ),
                                Span::styled(" │ ", self.theme.style(Component::DimText)),
                                Span::styled(
                                    right_prefix,
                                    self.theme.style(Component::CodeAddition),
                                ),
                                Span::styled(
                                    right,
                                    if right_content.is_empty() {
                                        self.theme.style(Component::DimText)
                                    } else {
                                        self.theme.style(Component::CodeAddition)
                                    },
                                ),
                            ]));
                        }

                        i = j;
                    }
                }
                ChangeTag::Insert => {
                    // Pure insertion (not part of replacement) - show on right only
                    let content = change.value().trim_end();
                    let left = " ".repeat(half_width);
                    let right = self.truncate_or_pad(content, half_width);

                    lines.push(Line::from(vec![
                        Span::styled(" ", self.theme.style(Component::DimText)),
                        Span::styled(left, self.theme.style(Component::DimText)),
                        Span::styled(" │ ", self.theme.style(Component::DimText)),
                        Span::styled("+", self.theme.style(Component::CodeAddition)),
                        Span::styled(right, self.theme.style(Component::CodeAddition)),
                    ]));
                    i += 1;
                }
            }

            // Check line limit
            if let Some(max) = self.max_lines {
                if lines.len() >= max {
                    break;
                }
            }
        }

        lines
    }

    fn format_line(&self, prefix: &str, content: &str, style: Style) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Wrap long lines
        let wrapped = textwrap::wrap(content, self.wrap_width.saturating_sub(2));

        if wrapped.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(" ", style),
            ]));
        } else {
            for (i, wrapped_line) in wrapped.iter().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(prefix.to_string(), style),
                        Span::styled(format!(" {wrapped_line}"), style),
                    ]));
                } else {
                    // Continuation lines
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled(wrapped_line.to_string(), style),
                    ]));
                }
            }
        }

        lines
    }

    fn separator_line(&self) -> Line<'static> {
        Line::from(Span::styled(
            "···",
            self.theme
                .style(Component::DimText)
                .add_modifier(Modifier::DIM),
        ))
    }

    fn truncate_or_pad(&self, s: &str, width: usize) -> String {
        // Use Unicode-aware truncation
        let char_count = s.chars().count();
        if char_count > width {
            // Collect chars and truncate at character boundary
            let truncated: String = s.chars().take(width.saturating_sub(1)).collect();
            format!("{truncated}…")
        } else {
            // Pad with spaces to reach desired width
            format!("{s:width$}")
        }
    }
}

// Helper function for preview/summary use cases
pub fn diff_summary(old: &str, new: &str, max_len: usize) -> (String, String) {
    let old_preview = if old.is_empty() {
        String::new()
    } else {
        let trimmed = old.trim();
        if trimmed.len() <= max_len {
            trimmed.to_string()
        } else {
            format!("{}...", &trimmed[..max_len.saturating_sub(3)])
        }
    };

    let new_preview = if new.is_empty() {
        String::new()
    } else {
        let trimmed = new.trim();
        if trimmed.len() <= max_len {
            trimmed.to_string()
        } else {
            format!("{}...", &trimmed[..max_len.saturating_sub(3)])
        }
    };

    (old_preview, new_preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;

    fn extract_text_from_line(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn test_unified_diff_basic() {
        let theme = Theme::default();
        let widget = DiffWidget::new("hello\nworld", "hello\nthere", &theme);
        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();

        let expected = vec!["  hello", "- world", "+ there"];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_split_diff_equal_lines() {
        let theme = Theme::default();
        let old = "line1\nline2\nline3";
        let new = "line1\nmodified2\nline3";

        let widget = DiffWidget::new(old, new, &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(80);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec![
            " line1                                 │  line1                                ",
            "-line2                                 │ +modified2                            ",
            " line3                                 │  line3                                ",
        ];

        assert_eq!(lines.len(), expected.len());
        assert_eq!(lines, expected);
    }

    #[test]
    fn test_split_diff_more_deletes_than_inserts() {
        let theme = Theme::default();
        let old = "line1\nline2\nline3\nline4\nline5";
        let new = "line1\nreplacement";

        let widget = DiffWidget::new(old, new, &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(80);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec![
            " line1                                 │  line1                                ",
            "-line2                                 │ +replacement                          ",
            "-line3                                 │                                       ",
            "-line4                                 │                                       ",
            "-line5                                 │                                       ",
        ];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_split_diff_more_inserts_than_deletes() {
        let theme = Theme::default();
        let old = "line1\nold";
        let new = "line1\nnew1\nnew2\nnew3";

        let widget = DiffWidget::new(old, new, &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(80);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec![
            " line1                                 │  line1                                ",
            "-old                                   │ +new1                                 ",
            "                                       │ +new2                                 ",
            "                                       │ +new3                                 ",
        ];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_unicode_truncation() {
        let theme = Theme::default();
        let old = "Short";
        let new = "This is a line with unicode: → ← ↑ ↓ — and more symbols";

        let widget = DiffWidget::new(old, new, &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(40); // Force truncation

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        // half_width = (40 - 5) / 2 = 17
        let expected = vec!["-Short             │ +This is a line w…"];
        assert_eq!(lines, expected);
    }

    #[test]
    fn test_narrow_terminal_fallback() {
        let theme = Theme::default();
        let widget = DiffWidget::new("old", "new", &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(30); // Too narrow for split view

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec!["- old", "+ new"];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_context_radius() {
        let theme = Theme::default();
        let old = "a\nb\nc\nd\ne\nf\ng";
        let new = "a\nb\nc\nX\ne\nf\ng";

        let widget = DiffWidget::new(old, new, &theme).with_context_radius(1); // Only 1 line of context

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        // With context_radius 1, skips a,b then shows c,d->X,e
        let expected = vec!["···", "  c", "- d", "+ X", "  e"];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_max_lines_limit() {
        let theme = Theme::default();
        let old = "line 0\nline 1\nline 2\nline 3\nline 4\nline 5";
        let new = "modified 0\nmodified 1\nmodified 2\nmodified 3\nmodified 4\nmodified 5";

        let widget = DiffWidget::new(old, new, &theme).with_max_lines(10);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec![
            "- line 0",
            "- line 1",
            "- line 2",
            "- line 3",
            "- line 4",
            "- line 5",
            "+ modified 0",
            "+ modified 1",
            "+ modified 2",
            "+ modified 3",
            "... (2 more lines)",
        ];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_diff_summary() {
        let (old_preview, new_preview) = diff_summary(
            "This is a very long line that should be truncated",
            "Short",
            20,
        );

        assert_eq!(old_preview, "This is a very lo...");
        assert_eq!(new_preview, "Short");
    }

    #[test]
    fn test_empty_strings() {
        let theme = Theme::default();

        // Test empty old string (pure addition)
        let widget = DiffWidget::new("", "new content", &theme);
        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec!["+ new content"];
        assert_eq!(lines, expected);

        // Test empty new string (pure deletion)
        let widget = DiffWidget::new("old content", "", &theme);
        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec!["- old content"];
        assert_eq!(lines, expected);

        // Test both empty
        let widget = DiffWidget::new("", "", &theme);
        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        assert!(lines.is_empty());
    }

    #[test]
    fn test_line_wrapping() {
        let theme = Theme::default();
        let old = "short";
        let new = "This is a very long line that should be wrapped when displayed in the diff widget because it exceeds the wrap width";

        let widget = DiffWidget::new(old, new, &theme).with_wrap_width(30);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        // wrap_width=30, so 28 chars per line
        let expected = vec![
            "- short",
            "+ This is a very long line",
            "  that should be wrapped when",
            "  displayed in the diff widget",
            "  because it exceeds the wrap",
            "  width",
        ];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_unified_diff_exact_output() {
        let theme = Theme::default();
        let widget = DiffWidget::new("line1\nline2\nline3", "line1\nmodified\nline3", &theme)
            .with_context_radius(1);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        // With context_radius 1, we show line1 (context), line2->modified (change), line3 (context)
        // No lines are skipped, so no separator
        let expected = vec!["  line1", "- line2", "+ modified", "  line3"];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_split_diff_exact_output() {
        let theme = Theme::default();
        let widget = DiffWidget::new("same\nold\nsame", "same\nnew\nsame", &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(80); // Wide enough to not trigger fallback

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec![
            " same                                  │  same                                 ",
            "-old                                   │ +new                                  ",
            " same                                  │  same                                 ",
        ];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_split_diff_uneven_replacement_exact() {
        let theme = Theme::default();
        let widget = DiffWidget::new("a\nb\nc\nd\ne", "a\nX\ne", &theme)
            .with_mode(DiffMode::Split)
            .with_wrap_width(80);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec![
            " a                                     │  a                                    ",
            "-b                                     │ +X                                    ",
            "-c                                     │                                       ",
            "-d                                     │                                       ",
            " e                                     │  e                                    ",
        ];
        assert_eq!(lines, expected);
    }

    #[test]
    fn test_context_radius_exact() {
        let theme = Theme::default();
        let widget = DiffWidget::new(
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            "1\n2\n3\n4\nX\n6\n7\n8\n9",
            &theme,
        )
        .with_context_radius(2);

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        // With context radius 2, skip 1,2, show 3,4,5->X,6,7, skip 8,9
        let expected = vec!["···", "  3", "  4", "- 5", "+ X", "  6", "  7"];

        assert_eq!(lines, expected);
    }

    #[test]
    fn test_line_wrapping_exact() {
        let theme = Theme::default();
        let widget =
            DiffWidget::new("short", "This is a long line that wraps", &theme).with_wrap_width(20); // Force wrapping

        let lines = widget
            .lines()
            .iter()
            .map(extract_text_from_line)
            .collect::<Vec<_>>();
        let expected = vec!["- short", "+ This is a long", "  line that wraps"];

        assert_eq!(lines, expected);
    }
}
