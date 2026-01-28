use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap,
    },
};

use crate::tui::theme::{Component, Theme};

const PREVIEW_LINES: usize = 8;

#[derive(Debug, Default)]
pub struct EditSelectionOverlayState {
    pub messages: Vec<(String, String)>,
    pub selected_index: usize,
}

impl EditSelectionOverlayState {
    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if self.selected_index + 1 < self.messages.len() {
            self.selected_index += 1;
        }
    }

    pub fn get_selected(&self) -> Option<&(String, String)> {
        self.messages.get(self.selected_index)
    }

    pub fn populate(&mut self, messages: Vec<(String, String)>) {
        self.messages = messages;
        if self.messages.is_empty() {
            self.selected_index = 0;
        } else {
            self.selected_index = self.messages.len() - 1;
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.selected_index = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

pub struct EditSelectionOverlay<'a> {
    theme: &'a Theme,
}

impl<'a> EditSelectionOverlay<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }

    fn centered_rect(area: Rect) -> Rect {
        let width_percent = 70;
        let height_percent = 60;

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - height_percent) / 2),
                Constraint::Percentage(height_percent),
                Constraint::Percentage((100 - height_percent) / 2),
            ])
            .split(area);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - width_percent) / 2),
                Constraint::Percentage(width_percent),
                Constraint::Percentage((100 - width_percent) / 2),
            ])
            .split(vertical[1])[1]
    }

    fn calculate_window(total: usize, selected: usize, max_visible: usize) -> (usize, usize) {
        if total <= max_visible {
            (0, total)
        } else {
            let half_window = max_visible / 2;
            if selected < half_window {
                (0, max_visible)
            } else if selected >= total.saturating_sub(half_window) {
                (total - max_visible, total)
            } else {
                let start = selected - half_window;
                (start, start + max_visible)
            }
        }
    }

    fn format_snippet(content: &str, max_width: usize) -> String {
        content
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(max_width)
            .collect()
    }

    fn format_preview(content: &str, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut current_line = String::new();

        for ch in content.chars() {
            if ch == '\n' {
                lines.push(Line::from(current_line.clone()));
                current_line.clear();
                if lines.len() >= PREVIEW_LINES {
                    break;
                }
            } else {
                current_line.push(ch);
                if current_line.len() >= width {
                    lines.push(Line::from(current_line.clone()));
                    current_line.clear();
                    if lines.len() >= PREVIEW_LINES {
                        break;
                    }
                }
            }
        }

        if !current_line.is_empty() && lines.len() < PREVIEW_LINES {
            lines.push(Line::from(current_line));
        }

        lines
    }
}

impl StatefulWidget for EditSelectionOverlay<'_> {
    type State = EditSelectionOverlayState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let popup_area = Self::centered_rect(area);

        Clear.render(popup_area, buf);

        let outer_block = Block::default()
            .borders(Borders::ALL)
            .title(" Edit Message ")
            .style(self.theme.style(Component::InputPanelBorder))
            .border_style(self.theme.style(Component::InputPanelBorderActive));

        let inner_area = outer_block.inner(popup_area);
        outer_block.render(popup_area, buf);

        if state.is_empty() {
            let empty_msg = Paragraph::new("No user messages to edit")
                .style(self.theme.style(Component::DimText));
            empty_msg.render(inner_area, buf);
            return;
        }

        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(inner_area);

        let list_area = panes[0];
        let preview_area = panes[1];

        let list_width = list_area.width.saturating_sub(2) as usize;
        let max_visible = list_area.height.max(1) as usize;
        let (start_idx, end_idx) =
            Self::calculate_window(state.messages.len(), state.selected_index, max_visible);

        let items: Vec<ListItem> = state.messages[start_idx..end_idx]
            .iter()
            .map(|(_, content)| {
                let snippet = Self::format_snippet(content, list_width);
                ListItem::new(Line::from(snippet))
            })
            .collect();

        let mut list_state = ListState::default();
        list_state.select(Some(state.selected_index.saturating_sub(start_idx)));

        let highlight_style = self
            .theme
            .style(Component::SelectionHighlight)
            .add_modifier(Modifier::REVERSED);

        let list_block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(self.theme.style(Component::DimText));

        let list = List::new(items)
            .block(list_block)
            .highlight_style(highlight_style);

        StatefulWidget::render(list, list_area, buf, &mut list_state);

        let preview_width = preview_area.width.saturating_sub(2) as usize;
        let preview_content = state
            .get_selected()
            .map(|(_, content)| Self::format_preview(content, preview_width))
            .unwrap_or_default();

        let preview_block = Block::default()
            .borders(Borders::NONE)
            .padding(ratatui::widgets::Padding::horizontal(1));

        let preview = Paragraph::new(preview_content)
            .block(preview_block)
            .wrap(Wrap { trim: false });

        preview.render(preview_area, buf);

        let hint_area = Rect {
            x: popup_area.x + 1,
            y: popup_area.y + popup_area.height.saturating_sub(1),
            width: popup_area.width.saturating_sub(2),
            height: 1,
        };

        let hint = Line::from(vec![
            Span::styled("[↑↓]", self.theme.style(Component::InputPanelLabelActive)),
            Span::styled(" navigate ", self.theme.style(Component::DimText)),
            Span::styled(
                "[Enter]",
                self.theme.style(Component::InputPanelLabelActive),
            ),
            Span::styled(" select ", self.theme.style(Component::DimText)),
            Span::styled("[Esc]", self.theme.style(Component::InputPanelLabelActive)),
            Span::styled(" cancel", self.theme.style(Component::DimText)),
        ]);

        buf.set_line(hint_area.x, hint_area.y, &hint, hint_area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_navigation() {
        let mut state = EditSelectionOverlayState::default();
        state.populate(vec![
            ("id1".to_string(), "First message".to_string()),
            ("id2".to_string(), "Second message".to_string()),
            ("id3".to_string(), "Third message".to_string()),
        ]);

        assert_eq!(state.selected_index, 2);

        state.select_prev();
        assert_eq!(state.selected_index, 1);

        state.select_prev();
        assert_eq!(state.selected_index, 0);

        state.select_prev();
        assert_eq!(state.selected_index, 0);

        state.select_next();
        assert_eq!(state.selected_index, 1);

        state.select_next();
        assert_eq!(state.selected_index, 2);

        state.select_next();
        assert_eq!(state.selected_index, 2);
    }

    #[test]
    fn test_empty_state() {
        let mut state = EditSelectionOverlayState::default();
        assert!(state.is_empty());
        assert!(state.get_selected().is_none());

        state.populate(vec![]);
        assert!(state.is_empty());
    }

    #[test]
    fn test_format_snippet() {
        let content = "First line\nSecond line\nThird line";
        let snippet = EditSelectionOverlay::format_snippet(content, 20);
        assert_eq!(snippet, "First line");

        let long_content = "This is a very long first line that should be truncated";
        let truncated = EditSelectionOverlay::format_snippet(long_content, 20);
        assert_eq!(truncated.len(), 20);
    }

    #[test]
    fn test_format_preview() {
        let content =
            "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10";
        let preview = EditSelectionOverlay::format_preview(content, 80);
        assert_eq!(preview.len(), 8);
    }
}
