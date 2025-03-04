use ratatui::text::{Line, Span, Text};
use ratatui::style::{Color, Style, Modifier};
use syntect::easy::HighlightLines;
use syntect::highlighting::{ThemeSet, Style as SyntectStyle};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

/// Format a message for display in the terminal
pub fn format_message(content: &str, role: crate::app::Role) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_block_content = String::new();
    let mut language = String::new();
    
    // Initialize syntax highlighting
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];
    
    // Process the message line by line
    for line in content.lines() {
        // Check for code block delimiters
        if line.starts_with("```") {
            if in_code_block {
                // End of code block
                in_code_block = false;
                
                // Syntax highlight the code block
                if !code_block_content.is_empty() {
                    let syntax = if language.is_empty() {
                        ps.find_syntax_plain_text()
                    } else {
                        ps.find_syntax_by_token(&language).unwrap_or_else(|| ps.find_syntax_plain_text())
                    };
                    
                    let mut highlighter = HighlightLines::new(syntax, theme);
                    
                    for code_line in code_block_content.lines() {
                        let highlighted = highlighter.highlight_line(code_line, &ps).unwrap_or_default();
                        let mut spans: Vec<Span> = Vec::new();
                        
                        for (style, text) in highlighted {
                            let color = convert_syntect_style_to_color(&style);
                            spans.push(Span::styled(text.to_string(), Style::default().fg(color)));
                        }
                        
                        lines.push(Line::from(spans));
                    }
                }
                
                code_block_content.clear();
                language.clear();
            } else {
                // Start of code block
                in_code_block = true;
                language = line.trim_start_matches("```").to_string();
            }
        } else if in_code_block {
            // Inside code block
            code_block_content.push_str(line);
            code_block_content.push('\n');
        } else {
            // Regular text
            lines.push(Line::from(Span::raw(line.to_string())));
        }
    }
    
    lines
}

/// Convert a syntect style to a ratatui color
fn convert_syntect_style_to_color(style: &SyntectStyle) -> Color {
    if style.foreground.a == 0 {
        return Color::Reset;
    }
    
    Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    )
}