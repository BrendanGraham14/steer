//! Theme-aware markdown renderer for the TUI
//!
//! This is a modified version of tui-markdown that accepts theme styles
//! instead of using hardcoded ones.

use itertools::{Itertools, Position};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use tracing::{debug, instrument, warn};
use unicode_width::UnicodeWidthStr;

use crate::tui::theme::{Component, Theme};

/// Markdown styles that can be customized via the theme
#[derive(Debug, Clone)]
pub struct MarkdownStyles {
    pub h1: Style,
    pub h2: Style,
    pub h3: Style,
    pub h4: Style,
    pub h5: Style,
    pub h6: Style,
    pub emphasis: Style,
    pub strong: Style,
    pub strikethrough: Style,
    pub blockquote: Style,
    pub code: Style,
    pub code_block: Style,
    pub link: Style,
    pub list_marker: Style,
    pub list_number: Style,
    pub table_border: Style,
    pub table_header: Style,
    pub table_cell: Style,
    pub task_checked: Style,
    pub task_unchecked: Style,
}

impl MarkdownStyles {
    /// Create markdown styles from a theme
    pub fn from_theme(theme: &Theme) -> Self {
        use ratatui::style::Modifier;

        Self {
            // Headings - add semantic modifiers on top of theme colors
            h1: theme
                .style(Component::MarkdownH1)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            h2: theme
                .style(Component::MarkdownH2)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            h3: theme
                .style(Component::MarkdownH3)
                .add_modifier(Modifier::BOLD),
            h4: theme
                .style(Component::MarkdownH4)
                .add_modifier(Modifier::UNDERLINED),
            h5: theme
                .style(Component::MarkdownH5)
                .add_modifier(Modifier::ITALIC),
            h6: theme
                .style(Component::MarkdownH6)
                .add_modifier(Modifier::ITALIC),

            // Text modifiers - these are purely semantic, ignore theme styles
            emphasis: Style::default().add_modifier(Modifier::ITALIC),
            strong: Style::default().add_modifier(Modifier::BOLD),
            strikethrough: Style::default().add_modifier(Modifier::CROSSED_OUT),

            // Other elements - add semantic modifiers where appropriate
            blockquote: theme
                .style(Component::MarkdownBlockquote)
                .add_modifier(Modifier::ITALIC),
            code: theme.style(Component::MarkdownCode),
            code_block: theme.style(Component::MarkdownCodeBlock),
            link: theme
                .style(Component::MarkdownLink)
                .add_modifier(Modifier::UNDERLINED),
            list_marker: theme.style(Component::MarkdownListBullet),
            list_number: theme.style(Component::MarkdownListNumber),
            table_border: theme.style(Component::MarkdownTableBorder),
            table_header: theme.style(Component::MarkdownTableHeader),
            table_cell: theme.style(Component::MarkdownTableCell),
            task_checked: theme.style(Component::MarkdownTaskChecked),
            task_unchecked: theme.style(Component::MarkdownTaskUnchecked),
        }
    }
}

pub fn from_str<'a>(input: &'a str, styles: &'a MarkdownStyles) -> Text<'a> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    let parser = Parser::new_ext(input, options);
    let mut writer = TextWriter::new(parser, styles);
    writer.run();
    writer.text
}

struct TextWriter<'a, I> {
    /// Iterator supplying events.
    iter: I,

    /// Text to write to.
    text: Text<'a>,

    /// Current style.
    ///
    /// This is a stack of styles, with the top style being the current style.
    inline_styles: Vec<Style>,

    /// Prefix to add to the start of the each line.
    line_prefixes: Vec<Span<'a>>,

    /// Stack of line styles.
    line_styles: Vec<Style>,

    /// Current list index as a stack of indices.
    list_indices: Vec<Option<u64>>,

    /// A link which will be appended to the current line when the link tag is closed.
    link: Option<CowStr<'a>>,

    needs_newline: bool,

    /// The markdown styles to use
    styles: &'a MarkdownStyles,

    /// Table state
    table_alignments: Vec<pulldown_cmark::Alignment>,
    table_rows: Vec<Vec<Vec<Span<'a>>>>, // rows of cells, each cell is a vec of spans
    in_table_header: bool,

    /// Track if we just started a list item (for task list markers)
    in_list_item_start: bool,
}

impl<'a, I> TextWriter<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    fn new(iter: I, styles: &'a MarkdownStyles) -> Self {
        Self {
            iter,
            text: Text::default(),
            inline_styles: vec![],
            line_styles: vec![],
            line_prefixes: vec![],
            list_indices: vec![],
            needs_newline: false,
            link: None,
            styles,
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
            in_table_header: false,
            in_list_item_start: false,
        }
    }

    fn run(&mut self) {
        debug!("Running text writer");
        while let Some(event) = self.iter.next() {
            self.handle_event(event);
        }
    }

    #[instrument(level = "debug", skip(self))]
    fn handle_event(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            Event::Html(html) => warn!("Html not yet supported: {}", html),
            Event::FootnoteReference(_) => warn!("Footnote reference not yet supported"),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => self.rule(),
            Event::TaskListMarker(checked) => self.task_list_marker(checked),
        }
    }

    fn start_tag(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading(level, _, _) => self.start_heading(level),
            Tag::BlockQuote => self.start_blockquote(),
            Tag::CodeBlock(kind) => self.start_codeblock(kind),
            Tag::List(start_index) => self.start_list(start_index),
            Tag::Item => self.start_item(),
            Tag::FootnoteDefinition(_) => warn!("Footnote definition not yet supported"),
            Tag::Table(alignments) => self.start_table(alignments),
            Tag::TableHead => self.start_table_head(),
            Tag::TableRow => self.start_table_row(),
            Tag::TableCell => self.start_table_cell(),
            Tag::Emphasis => self.push_inline_style(self.styles.emphasis),
            Tag::Strong => self.push_inline_style(self.styles.strong),
            Tag::Strikethrough => self.push_inline_style(self.styles.strikethrough),
            Tag::Link(_link_type, dest_url, _title) => self.push_link(dest_url),
            Tag::Image(_link_type, _dest_url, _title) => warn!("Image not yet supported"),
        }
    }

    fn end_tag(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.end_paragraph(),
            Tag::Heading(..) => self.end_heading(),
            Tag::BlockQuote => self.end_blockquote(),
            Tag::CodeBlock(_) => self.end_codeblock(),
            Tag::List(_) => self.end_list(),
            Tag::Item => self.end_item(),
            Tag::FootnoteDefinition(_) => {}
            Tag::Table(_) => self.end_table(),
            Tag::TableHead => self.end_table_head(),
            Tag::TableRow => self.end_table_row(),
            Tag::TableCell => self.end_table_cell(),
            Tag::Emphasis => self.pop_inline_style(),
            Tag::Strong => self.pop_inline_style(),
            Tag::Strikethrough => self.pop_inline_style(),
            Tag::Link(..) => self.pop_link(),
            Tag::Image(..) => {}
        }
    }

    fn start_paragraph(&mut self) {
        // Insert an empty line between paragraphs if there is at least one line of text already.
        if self.needs_newline {
            self.push_line(Line::default());
        }
        self.push_line(Line::default());
        self.needs_newline = false;
    }

    fn end_paragraph(&mut self) {
        self.needs_newline = true
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        let style = match level {
            HeadingLevel::H1 => self.styles.h1,
            HeadingLevel::H2 => self.styles.h2,
            HeadingLevel::H3 => self.styles.h3,
            HeadingLevel::H4 => self.styles.h4,
            HeadingLevel::H5 => self.styles.h5,
            HeadingLevel::H6 => self.styles.h6,
        };
        // Push the heading style so it applies to the text content
        self.push_inline_style(style);

        let content = format!("{} ", "#".repeat(level as usize));
        self.push_line(Line::styled(content, style));
        self.needs_newline = false;
    }

    fn end_heading(&mut self) {
        // Pop the heading style we pushed in start_heading
        self.pop_inline_style();
        self.needs_newline = true
    }

    fn start_blockquote(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
            self.needs_newline = false;
        }
        self.line_prefixes.push(Span::from(">"));
        self.line_styles.push(self.styles.blockquote);
    }

    fn end_blockquote(&mut self) {
        self.line_prefixes.pop();
        self.line_styles.pop();
        self.needs_newline = true;
    }

    fn text(&mut self, text: CowStr<'a>) {
        // If we're at the start of a list item and haven't seen a task list marker,
        // push the regular list marker
        if self.in_list_item_start {
            self.push_list_marker();
            self.in_list_item_start = false;
        }

        // Check if we're in a table cell
        let in_table =
            self.table_rows.last().is_some() && self.table_rows.last().unwrap().last().is_some();

        if in_table {
            // If we're in a table, just add the text as a span to the current cell
            let style = self.inline_styles.last().copied().unwrap_or_default();
            let span = Span::styled(text.to_string(), style);
            self.push_span(span);
        } else {
            // Original behavior for non-table text
            for (position, line) in text.lines().with_position() {
                if self.needs_newline {
                    self.push_line(Line::default());
                    self.needs_newline = false;
                }
                if matches!(position, Position::Middle | Position::Last) {
                    self.push_line(Line::default());
                }

                let style = self.inline_styles.last().copied().unwrap_or_default();
                let span = Span::styled(line.to_owned(), style);
                self.push_span(span);
            }
            self.needs_newline = false;
        }
    }

    fn code(&mut self, code: CowStr<'a>) {
        let span = Span::styled(code, self.styles.code);
        self.push_span(span);
    }

    fn hard_break(&mut self) {
        // Hard break should add a line break but stay in the same paragraph
        self.push_span("  ".into()); // Two spaces to visually indicate hard break
        self.push_line(Line::default());
    }

    fn start_list(&mut self, index: Option<u64>) {
        if self.list_indices.is_empty() && self.needs_newline {
            self.push_line(Line::default());
        }
        self.list_indices.push(index);
    }

    fn end_list(&mut self) {
        self.list_indices.pop();
        self.needs_newline = true;
    }

    fn start_item(&mut self) {
        self.push_line(Line::default());
        // Mark that we're at the start of a list item
        self.in_list_item_start = true;
        // Don't push the list marker yet - wait for task list marker if present
        self.needs_newline = false;
    }

    fn end_item(&mut self) {
        // If we still have in_list_item_start set, it means we had an empty list item
        // We need to push the list marker for empty items
        if self.in_list_item_start {
            self.push_list_marker();
            self.in_list_item_start = false;
        }
    }

    fn soft_break(&mut self) {
        // Soft break: In markdown, this is typically rendered as a space
        // unless it's at the end of a line
        if let Some(line) = self.text.lines.last() {
            if !line.spans.is_empty() {
                // Add a space if there's content on the current line
                self.push_span(" ".into());
            }
        }
    }

    fn start_codeblock(&mut self, kind: CodeBlockKind<'_>) {
        if !self.text.lines.is_empty() {
            self.push_line(Line::default());
        }
        let lang = match kind {
            CodeBlockKind::Fenced(ref lang) => lang.as_ref(),
            CodeBlockKind::Indented => "",
        };

        self.line_styles.push(self.styles.code_block);

        let span = Span::from(format!("```{lang}"));
        self.push_line(span.into());
        self.needs_newline = true;
    }

    fn end_codeblock(&mut self) {
        let span = Span::from("```");
        self.push_line(span.into());
        self.needs_newline = true;

        self.line_styles.pop();
    }

    #[instrument(level = "trace", skip(self))]
    fn push_inline_style(&mut self, style: Style) {
        let current_style = self.inline_styles.last().copied().unwrap_or_default();
        let style = current_style.patch(style);
        self.inline_styles.push(style);
        debug!("Pushed inline style: {:?}", style);
        debug!("Current inline styles: {:?}", self.inline_styles);
    }

    #[instrument(level = "trace", skip(self))]
    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    #[instrument(level = "trace", skip(self))]
    fn push_line(&mut self, line: Line<'a>) {
        let style = self.line_styles.last().copied().unwrap_or_default();
        let mut line = line.patch_style(style);

        // Add line prefixes to the start of the line.
        let line_prefixes = self.line_prefixes.iter().cloned().collect_vec();
        let has_prefixes = !line_prefixes.is_empty();
        if has_prefixes {
            line.spans.insert(0, " ".into());
        }
        for prefix in line_prefixes.iter().rev().cloned() {
            line.spans.insert(0, prefix);
        }
        self.text.lines.push(line);
    }

    #[instrument(level = "trace", skip(self))]
    fn push_span(&mut self, span: Span<'a>) {
        // Check if we're in a table cell first
        let in_table =
            self.table_rows.last().is_some() && self.table_rows.last().unwrap().last().is_some();

        if in_table {
            // We know we have a table row and cell, so we can safely unwrap
            let current_row = self.table_rows.last_mut().unwrap();
            let current_cell = current_row.last_mut().unwrap();
            current_cell.push(span);
        } else if let Some(line) = self.text.lines.last_mut() {
            line.push_span(span);
        } else {
            self.push_line(Line::from(vec![span]));
        }
    }

    /// Store the link to be appended to the link text
    #[instrument(level = "trace", skip(self))]
    fn push_link(&mut self, dest_url: CowStr<'a>) {
        self.link = Some(dest_url);
    }

    /// Append the link to the current line
    #[instrument(level = "trace", skip(self))]
    fn pop_link(&mut self) {
        if let Some(link) = self.link.take() {
            self.push_span(" (".into());
            self.push_span(Span::styled(link, self.styles.link));
            self.push_span(")".into());
        }
    }

    // Table handling methods

    fn start_table(&mut self, alignments: Vec<pulldown_cmark::Alignment>) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        self.table_alignments = alignments;
        self.table_rows.clear();
        self.needs_newline = false;
    }

    fn end_table(&mut self) {
        self.render_table();
        self.table_alignments.clear();
        self.table_rows.clear();
        self.needs_newline = true;
    }

    fn start_table_head(&mut self) {
        self.in_table_header = true;
        // Create a row for the header cells
        self.table_rows.push(Vec::new());
    }

    fn end_table_head(&mut self) {
        self.in_table_header = false;
    }

    fn start_table_row(&mut self) {
        self.table_rows.push(Vec::new());
    }

    fn end_table_row(&mut self) {
        // Nothing to do here, row is already added
    }

    fn start_table_cell(&mut self) {
        // Push a new cell to the current row
        if let Some(current_row) = self.table_rows.last_mut() {
            current_row.push(Vec::new());
        }
    }

    fn end_table_cell(&mut self) {
        // Nothing to do here, cell is already added
    }

    /// Render the accumulated table with proper alignment
    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        // Move rows out of `self` to avoid borrow conflicts during rendering
        let rows = std::mem::take(&mut self.table_rows);

        // Calculate column widths
        let num_cols = self.table_alignments.len();
        let mut col_widths = vec![0; num_cols];

        for row in &rows {
            for (col_idx, cell) in row.iter().enumerate() {
                if col_idx < num_cols {
                    let cell_width = cell
                        .iter()
                        .map(|span| span.content.as_ref().width())
                        .sum::<usize>();
                    col_widths[col_idx] = col_widths[col_idx].max(cell_width);
                }
            }
        }

        // Add padding to column widths
        for width in &mut col_widths {
            *width += 2; // Add 1 space padding on each side
        }

        // Render the table
        let border_style = self.styles.table_border;
        let header_style = self.styles.table_header;
        let cell_style = self.styles.table_cell;

        // Top border
        self.render_table_border(&col_widths, '┌', '┬', '┐', border_style);

        // Render rows
        for (row_idx, row) in rows.iter().enumerate() {
            let is_header = row_idx == 0 && rows.len() > 1;
            let mut line_spans = vec![Span::styled("│", border_style)];

            for (col_idx, cell) in row.iter().enumerate() {
                if col_idx < num_cols {
                    // Concatenate cell spans into a single string for alignment
                    let cell_text: String = cell
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<Vec<_>>()
                        .join("");

                    let padded = self.align_text(
                        &cell_text,
                        col_widths[col_idx],
                        self.table_alignments[col_idx],
                    );

                    // Apply appropriate style
                    let style = if is_header { header_style } else { cell_style };
                    line_spans.push(Span::styled(padded, style));
                    line_spans.push(Span::styled("│", border_style));
                }
            }

            self.push_line(Line::from(line_spans));

            // Add separator after header
            if is_header {
                self.render_table_border(&col_widths, '├', '┼', '┤', border_style);
            }
        }

        // Bottom border
        self.render_table_border(&col_widths, '└', '┴', '┘', border_style);
    }

    /// Render a table border line
    fn render_table_border(
        &mut self,
        col_widths: &[usize],
        left: char,
        mid: char,
        right: char,
        style: Style,
    ) {
        let mut border = String::from(left);

        for (idx, &width) in col_widths.iter().enumerate() {
            border.push_str(&"─".repeat(width));
            if idx < col_widths.len() - 1 {
                border.push(mid);
            }
        }

        border.push(right);
        self.push_line(Line::from(Span::styled(border, style)));
    }

    /// Align text within a given width based on alignment
    fn align_text(&self, text: &str, width: usize, alignment: pulldown_cmark::Alignment) -> String {
        let text_len = text.width();
        // Total spaces needed = width - text_len
        // We already have 2 spaces in the format string (1 before, 1 after)
        let total_padding = width.saturating_sub(text_len);

        match alignment {
            pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => {
                // Left align: 1 space before, remaining spaces after
                let right_padding = total_padding.saturating_sub(1);
                format!(" {}{}", text, " ".repeat(right_padding))
            }
            pulldown_cmark::Alignment::Center => {
                // Center: distribute padding evenly
                let left_padding = total_padding / 2;
                let right_padding = total_padding - left_padding;
                format!(
                    "{}{}{}",
                    " ".repeat(left_padding),
                    text,
                    " ".repeat(right_padding)
                )
            }
            pulldown_cmark::Alignment::Right => {
                // Right align: remaining spaces before, 1 space after
                let left_padding = total_padding.saturating_sub(1);
                format!("{}{} ", " ".repeat(left_padding), text)
            }
        }
    }

    /// Render a horizontal rule
    fn rule(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
        }

        // Create a horizontal rule using box-drawing characters
        // We'll use a solid line of dashes or unicode box characters
        let terminal_width = 80; // Default width, could be made configurable
        let rule_char = "─"; // Unicode box drawing character
        let rule_content = rule_char.repeat(terminal_width.min(120)); // Cap at 120 chars

        // Use the blockquote style for rules (or we could add a dedicated rule style)
        let rule_style = self.styles.blockquote;
        self.push_line(Line::from(Span::styled(rule_content, rule_style)));

        self.needs_newline = true;
    }

    /// Push the appropriate list marker (bullet or number)
    fn push_list_marker(&mut self) {
        // If we're not inside a list, there's nothing to render – avoid underflow.
        if self.list_indices.is_empty() {
            return;
        }

        let width = self.list_indices.len().saturating_mul(4).saturating_sub(3);
        if let Some(last_index) = self.list_indices.last_mut() {
            let span = match last_index {
                None => Span::styled(
                    " ".repeat(width.saturating_sub(1)) + "- ",
                    self.styles.list_marker,
                ),
                Some(index) => {
                    *index += 1;
                    Span::styled(format!("{:width$}. ", *index - 1), self.styles.list_number)
                }
            };
            self.push_span(span);
        }
    }

    /// Render a task list marker (checkbox)
    fn task_list_marker(&mut self, checked: bool) {
        // If we're not inside a list, there's nothing to render – avoid underflow.
        if self.list_indices.is_empty() {
            return;
        }

        // Push the list indentation and marker
        let width = self.list_indices.len().saturating_mul(4).saturating_sub(3);
        let indent = " ".repeat(width.saturating_sub(1));

        // Use checkbox characters
        let checkbox = if checked { "[✓] " } else { "[ ] " };

        // Apply appropriate style based on checked state
        let style = if checked {
            self.styles.task_checked
        } else {
            self.styles.task_unchecked
        };

        let span = Span::styled(format!("{indent}- {checkbox}"), style);
        self.push_span(span);

        // Mark that we've handled the list item start
        self.in_list_item_start = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;
    use pulldown_cmark::{Event, Options, Parser};

    #[test]
    fn test_table_parsing() {
        let markdown = r#"## Test Results Table

| Test Suite | Status | Passed | Failed | Skipped | Duration |
|------------|--------|--------|--------|---------|----------|
| Unit Tests | ✅ | 247 | 0 | 3 | 2m 15s |
| Integration Tests | ✅ | 89 | 0 | 1 | 5m 42s |"#;

        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        let parser = Parser::new_ext(markdown, options);

        println!("=== Parser Events ===");
        for (idx, event) in parser.enumerate() {
            match &event {
                Event::Start(tag) => println!("{idx}: Start {tag:?}"),
                Event::End(tag) => println!("{idx}: End {tag:?}"),
                Event::Text(text) => println!("{idx}: Text: '{text}'"),
                _ => println!("{idx}: {event:?}"),
            }
        }
    }

    #[test]
    fn test_simple_table() {
        let markdown = r#"| Col1 | Col2 |
|------|------|
| A    | B    |"#;

        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        let parser = Parser::new_ext(markdown, options);

        println!("\n=== Simple Table Events ===");
        for (idx, event) in parser.enumerate() {
            match &event {
                Event::Start(tag) => println!("{idx}: Start {tag:?}"),
                Event::End(tag) => println!("{idx}: End {tag:?}"),
                Event::Text(text) => println!("{idx}: Text: '{text}'"),
                _ => println!("{idx}: {event:?}"),
            }
        }
    }

    #[test]
    fn test_table_rendering() {
        let markdown = r#"## Test Results Table

| Test Suite | Status | Passed | Failed | Skipped | Duration |
|------------|--------|--------|--------|---------|----------|
| Unit Tests | ✅ | 247 | 0 | 3 | 2m 15s |
| Integration Tests | ✅ | 89 | 0 | 1 | 5m 42s |"#;

        // Create a dummy theme for testing
        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);

        let rendered = from_str(markdown, &styles);

        println!("\n=== Rendered Output ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }
    }

    #[test]
    fn test_table_alignment() {
        let markdown = r#"| Left | Center | Right |
|:-----|:------:|------:|
| L    | C      | R     |
| Long Left Text | Centered | Right Aligned |"#;

        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        let parser = Parser::new_ext(markdown, options);

        println!("\n=== Alignment Test Events ===");
        for event in parser {
            if let Event::Start(Tag::Table(alignments)) = &event {
                println!("Table alignments: {alignments:?}");
            }
        }

        // Now test rendering
        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Rendered Table with Alignment ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }
    }

    #[test]
    fn test_table_edge_cases() {
        let markdown = r#"| Empty | Unicode | Mixed |
|-------|---------|-------|
|       | 你好 🌍   | Test  |
| A     |         | 123   |
|       |         |       |"#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Table with Edge Cases ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }

        // Test that all lines have content (no panic on empty cells)
        assert!(!rendered.lines.is_empty());
    }

    #[test]
    fn test_table_with_star_emojis() {
        let markdown = r#"## Complex Data Table

| ID  | Product       | Price   | Stock | Category     | Rating |
|-----|---------------|---------|-------|--------------|--------|
| 001 | MacBook Pro   | $2,399  | 12    | Electronics  | ⭐⭐⭐⭐⭐ |
| 002 | Coffee Mug    | $15.99  | 250   | Kitchen      | ⭐⭐⭐⭐   |
| 003 | Desk Chair    | $299.00 | 5     | Furniture    | ⭐⭐⭐     |"#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Table with Star Emojis ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }
    }

    #[test]
    fn test_line_breaks() {
        let markdown = r#"This is a line with a hard break  
at the end.

This is a soft break
that should become a space.

Multiple
soft
breaks
in
a
row."#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Line Breaks Test ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }
    }

    #[test]
    fn test_horizontal_rules() {
        let markdown = r#"Some text before

---

Some text after

* * *

Another section

___

Final section"#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Horizontal Rules Test ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }

        // Check that rules are present
        let has_rule = rendered
            .lines
            .iter()
            .any(|line| line.spans.iter().any(|span| span.content.contains("─")));
        assert!(has_rule, "Should contain horizontal rules");
    }

    #[test]
    fn test_task_lists() {
        let markdown = r#"## Todo List

- [x] Complete the parser implementation
- [ ] Add more tests
- [x] Write documentation
- [ ] Review code

Regular list items:
- Item 1
- Item 2

Mixed list:
1. [x] First task (done)
2. [ ] Second task (pending)
3. Regular numbered item"#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Task Lists Test ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }

        // Check that checkboxes are present
        let has_checked = rendered
            .lines
            .iter()
            .any(|line| line.spans.iter().any(|span| span.content.contains("[✓]")));
        let has_unchecked = rendered
            .lines
            .iter()
            .any(|line| line.spans.iter().any(|span| span.content.contains("[ ]")));
        assert!(has_checked, "Should contain checked checkboxes");
        assert!(has_unchecked, "Should contain unchecked checkboxes");
    }

    #[test]
    fn test_empty_list_items() {
        // Test #2: Empty list items that might leave in_list_item_start as true
        let markdown = r#"Empty list items:
- 
- Item with content
- 
- Another item

Empty numbered items:
1. 
2. Content here
3. "#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Empty List Items Test ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }

        // Ensure no panic occurred
        assert!(!rendered.lines.is_empty());
    }

    #[test]
    fn test_malformed_lists() {
        // Test #3: Various edge cases that might cause state issues
        let markdown = r#"List interrupted by other content:
- Item 1
This is a paragraph, not in the list
- Item 2

Nested list edge cases:
- Outer item
  - Inner item
  Some text here
- Back to outer

Task list edge cases:
- [ ] 
- [x] Task with content
- [ ] 

Mixed content:
1. [ ] Task in numbered list
Regular text
2. Another item"#;

        let theme = Theme::default();
        let styles = MarkdownStyles::from_theme(&theme);
        let rendered = from_str(markdown, &styles);

        println!("\n=== Malformed Lists Test ===");
        for (idx, line) in rendered.lines.iter().enumerate() {
            let line_text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            println!("Line {idx}: '{line_text}'");
        }

        // Ensure no panic occurred
        assert!(!rendered.lines.is_empty());
    }

    #[test]
    fn test_state_tracking_debug() {
        // Test with debug output to track state
        let markdown = r#"- Item 1
- 
- [ ] Task item
- 
Regular paragraph

- New list"#;

        let mut options = Options::empty();
        options.insert(Options::ENABLE_TASKLISTS);
        let parser = Parser::new_ext(markdown, options);

        println!("\n=== State Tracking Debug ===");

        // Create a custom writer to track state
        struct DebugTextWriter<'a, I> {
            inner: TextWriter<'a, I>,
        }

        let styles = MarkdownStyles::from_theme(&Theme::default());
        let mut writer = TextWriter::new(parser, &styles);

        // Manually process events to see state changes
        let parser = Parser::new_ext(markdown, options);
        for (idx, event) in parser.enumerate() {
            println!("Event {idx}: {event:?}");
            println!("  list_indices.len() = {}", writer.list_indices.len());
            println!("  in_list_item_start = {}", writer.in_list_item_start);
            writer.handle_event(event);
        }

        // Check final state
        println!("\nFinal state:");
        println!("  list_indices.len() = {}", writer.list_indices.len());
        println!("  in_list_item_start = {}", writer.in_list_item_start);

        assert_eq!(
            writer.list_indices.len(),
            0,
            "list_indices should be empty at end"
        );
        assert!(
            !writer.in_list_item_start,
            "in_list_item_start should be false at end"
        );
    }
}
