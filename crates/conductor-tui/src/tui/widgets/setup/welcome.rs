use crate::tui::theme::{Component, Theme};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use tui_big_text::{BigText, PixelSize};

pub struct WelcomeWidget;

impl WelcomeWidget {
    pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Create main block
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.style(Component::SetupBorder));
        let inner_area = block.inner(area);
        block.render(area, buf);

        // Split the area for BigText and regular text
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(30), // Flexible space above
                Constraint::Length(8),      // Space for BigText
                Constraint::Length(7),      // Space for text content
                Constraint::Percentage(70), // Flexible space below
            ])
            .split(inner_area);

        // Render BigText in the middle chunk
        let big_text = BigText::builder()
            .pixel_size(PixelSize::Full)
            .centered()
            .lines(vec![Line::from("Conductor")])
            .style(theme.style(Component::SetupBigText))
            .build();

        Widget::render(big_text, chunks[1], buf);

        // Render welcome message and instructions in the bottom chunk
        let lines = vec![
            // Add some spacing
            Line::from(""),
            // Add welcome message
            Line::from(vec![
                Span::raw("Welcome to "),
                Span::styled("Conductor", theme.style(Component::SetupTitle)),
                Span::raw("!"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Authenticate to get started.",
                theme.style(Component::SetupText),
            )),
            Line::from(""),
            Line::from(""),
            // Add instructions
            Line::from(vec![
                Span::raw("Press "),
                Span::styled("Enter", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to continue, or "),
                Span::styled("S", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to skip setup"),
            ]),
        ];

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });

        Widget::render(paragraph, chunks[2], buf);
    }
}
