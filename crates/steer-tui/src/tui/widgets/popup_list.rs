//! PopupList widget - A reusable modal list selector
//!
//! This widget provides a centered popup with a list of selectable items,
//! useful for model selection, branch selection, etc.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, StatefulWidget, Widget},
};
use std::fmt::Display;

/// A popup list widget for selecting from a list of items
#[derive(Debug)]
pub struct PopupList<'a, T> {
    /// Title of the popup
    title: &'a str,
    /// Items to display
    items: &'a [T],
    /// Style for the popup block
    block_style: Style,
    /// Style for selected item
    selected_style: Style,
}

impl<'a, T> PopupList<'a, T> {
    /// Create a new popup list
    pub fn new(title: &'a str, items: &'a [T]) -> Self {
        Self {
            title,
            items,
            block_style: Style::default().fg(Color::White),
            selected_style: Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        }
    }

    /// Set the style for the popup block
    pub fn block_style(mut self, style: Style) -> Self {
        self.block_style = style;
        self
    }

    /// Set the style for selected items
    pub fn selected_style(mut self, style: Style) -> Self {
        self.selected_style = style;
        self
    }

    /// Calculate the popup area (centered, 60% width, height based on content)
    pub fn centered_rect(area: Rect, max_height_percent: u16) -> Rect {
        let width_percent = 60;
        let _popup_width = area.width * width_percent / 100;

        // Calculate height based on content, but cap at max_height_percent
        let max_height = area.height * max_height_percent / 100;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - max_height_percent) / 2),
                Constraint::Max(max_height),
                Constraint::Percentage((100 - max_height_percent) / 2),
            ])
            .split(area);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - width_percent) / 2),
                Constraint::Percentage(width_percent),
                Constraint::Percentage((100 - width_percent) / 2),
            ])
            .split(chunks[1])[1]
    }
}

impl<T: Display> Widget for PopupList<'_, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the background
        Clear.render(area, buf);

        // Create the list items
        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|item| ListItem::new(Line::from(Span::raw(item.to_string()))))
            .collect();

        // Create the list widget
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.title)
                    .style(self.block_style),
            )
            .highlight_style(self.selected_style);

        // Render the list
        Widget::render(list, area, buf);
    }
}

/// State for the popup list (tracks selection)
#[derive(Debug, Clone)]
pub struct PopupListState {
    /// Currently selected index
    pub selected: usize,
    /// Total number of items
    total: usize,
}

impl PopupListState {
    /// Create a new popup list state
    pub fn new(total: usize) -> Self {
        Self { selected: 0, total }
    }

    /// Move selection up
    pub fn previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn next(&mut self) {
        if self.selected < self.total.saturating_sub(1) {
            self.selected += 1;
        }
    }

    /// Get the currently selected index
    pub fn selected(&self) -> usize {
        self.selected
    }
}

/// Stateful version of PopupList that maintains selection state
pub struct StatefulPopupList<'a, T> {
    popup: PopupList<'a, T>,
}

impl<'a, T: Display> StatefulPopupList<'a, T> {
    /// Create a new stateful popup list
    pub fn new(title: &'a str, items: &'a [T]) -> Self {
        Self {
            popup: PopupList::new(title, items),
        }
    }

    /// Set the style for the popup block
    pub fn block_style(mut self, style: Style) -> Self {
        self.popup = self.popup.block_style(style);
        self
    }

    /// Set the style for selected items
    pub fn selected_style(mut self, style: Style) -> Self {
        self.popup = self.popup.selected_style(style);
        self
    }
}

impl<T: Display> StatefulWidget for StatefulPopupList<'_, T> {
    type State = PopupListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Clear the background
        Clear.render(area, buf);

        // Create the list items
        let items: Vec<ListItem> = self
            .popup
            .items
            .iter()
            .map(|item| ListItem::new(Line::from(Span::raw(item.to_string()))))
            .collect();

        // Create list state for ratatui
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected));

        // Create the list widget
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.popup.title)
                    .style(self.popup.block_style),
            )
            .highlight_style(self.popup.selected_style);

        // Render the list
        StatefulWidget::render(list, area, buf, &mut list_state);
    }
}
