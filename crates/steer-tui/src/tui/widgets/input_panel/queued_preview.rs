use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, Widget};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::theme::{Component, Theme};

pub struct QueuedPreviewWidget<'a> {
    preview: Option<&'a str>,
    theme: &'a Theme,
}

impl<'a> QueuedPreviewWidget<'a> {
    pub fn new(preview: Option<&'a str>, theme: &'a Theme) -> Self {
        Self { preview, theme }
    }

    fn title(&self) -> Line<'static> {
        Line::from(Span::styled(
            "Queued",
            self.theme.style(Component::QueuedMessageLabel),
        ))
    }

    pub fn required_height(width: u16, preview: Option<&str>) -> u16 {
        let inner_width = width.saturating_sub(2).max(1) as usize;
        let text = preview.unwrap_or("");
        let mut line_count = 0usize;

        for line in text.lines() {
            let len = line.chars().count().max(1);
            line_count += len.div_ceil(inner_width);
        }

        if line_count == 0 {
            line_count = 1;
        }

        (line_count.saturating_add(2)).min(u16::MAX as usize) as u16
    }
}

impl Widget for QueuedPreviewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let preview = self.preview.unwrap_or("");
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.style(Component::QueuedMessageBorder))
            .title(self.title());
        let paragraph = Paragraph::new(preview)
            .style(self.theme.style(Component::QueuedMessageText))
            .wrap(Wrap { trim: false });
        let inner = block.inner(area);
        block.render(area, buf);
        paragraph.render(inner, buf);
    }
}
