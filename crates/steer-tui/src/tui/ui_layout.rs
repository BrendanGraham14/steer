//! UI Layout management for the TUI
//!
//! This module handles the layout computation and static widget rendering
//! to reduce complexity in the main draw loop.

use crate::tui::{theme::Theme, widgets::StatusBar};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::{Block, Clear},
};
use steer_grpc::client_api::ModelId;

/// Computed layout areas for the UI
pub struct UiLayout {
    /// The main chat area (includes border)
    pub chat: Rect,
    /// The input panel area
    pub input: Rect,
    /// The status bar area
    pub status: Rect,
    /// The full terminal area
    pub terminal: Rect,
}

impl UiLayout {
    /// Compute the layout based on terminal size, input requirements, and approval state
    pub fn compute(size: Rect, input_area_height: u16, _theme: &Theme) -> Self {
        // Main vertical layout: messages area, input area, status bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),                    // Messages area (flexible)
                Constraint::Length(input_area_height), // Input area (dynamic)
                Constraint::Length(1),                 // Status bar
            ])
            .split(size);

        Self {
            chat: chunks[0],
            input: chunks[1],
            status: chunks[2],
            terminal: size,
        }
    }

    /// Clear and prepare the background
    pub fn prepare_background(&self, f: &mut Frame, theme: &Theme) {
        // Clear the entire terminal area
        f.render_widget(Clear, self.terminal);

        // Apply background color if theme has one
        if let Some(bg_color) = theme.get_background_color() {
            let background_block = Block::default().style(Style::default().bg(bg_color));
            f.render_widget(background_block, self.terminal);
        }
    }

    /// Render the status bar
    pub fn render_status_bar(
        &self,
        f: &mut Frame,
        current_model: &ModelId,
        theme: &Theme,
        update_badge: crate::tui::widgets::status_bar::UpdateBadge,
    ) {
        let status_bar = StatusBar::new(current_model, theme).with_update_badge(update_badge);
        f.render_widget(status_bar, self.status);
    }
}
