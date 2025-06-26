use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};
use tui_textarea::TextArea;

/// Result of fuzzy finder operations
pub enum FuzzyFinderResult {
    /// User wants to close the finder
    Close,
    /// User selected a file
    Select(String),
}

/// A fuzzy finder component for file selection
pub struct FuzzyFinder {
    /// Whether the finder is currently active
    active: bool,
    /// Text input area for search query
    textarea: TextArea<'static>,
    /// Current search results
    results: Vec<String>,
    /// Currently selected result index
    selected: usize,
    /// List state for scrolling
    list_state: ListState,
}

impl Default for FuzzyFinder {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzyFinder {
    /// Create a new fuzzy finder
    pub fn new() -> Self {
        Self {
            active: false,
            textarea: TextArea::default(),
            results: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
        }
    }

    /// Check if the finder is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Activate the fuzzy finder
    pub fn activate(&mut self) {
        self.active = true;
        self.textarea = TextArea::default();
        self.selected = 0;
        self.results.clear();
        self.list_state = ListState::default();
    }

    /// Deactivate the fuzzy finder
    pub fn deactivate(&mut self) {
        self.active = false;
        self.textarea = TextArea::default();
        self.results.clear();
        self.selected = 0;
        self.list_state = ListState::default();
    }

    /// Get the current query
    pub fn query(&self) -> String {
        self.textarea.lines().join("")
    }

    /// Update the search results
    pub fn update_results(&mut self, results: Vec<String>) {
        self.results = results;
        // Reset selection if it's out of bounds
        if self.selected >= self.results.len() && !self.results.is_empty() {
            self.selected = 0;
        }
        // Update list state selection
        self.list_state.select(if self.results.is_empty() {
            None
        } else {
            Some(self.selected)
        });
    }

    /// Handle keyboard input
    pub fn handle_input(
        &mut self,
        key: ratatui::crossterm::event::KeyEvent,
    ) -> Option<FuzzyFinderResult> {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                // Cancel and close
                Some(FuzzyFinderResult::Close)
            }
            (KeyCode::Enter, _) => {
                // Select current item if available
                if !self.results.is_empty() && self.selected < self.results.len() {
                    let selected_path = self.results[self.selected].clone();
                    Some(FuzzyFinderResult::Select(selected_path))
                } else {
                    // Close if no results
                    Some(FuzzyFinderResult::Close)
                }
            }
            (KeyCode::Up, _) => {
                // Move selection up
                if self.selected > 0 {
                    self.selected -= 1;
                    self.list_state.select(Some(self.selected));
                }
                None
            }
            (KeyCode::Down, _) => {
                // Move selection down
                if self.selected + 1 < self.results.len() {
                    self.selected += 1;
                    self.list_state.select(Some(self.selected));
                }
                None
            }
            _ => {
                // Pass other keys to the textarea
                self.textarea.input(key);
                None
            }
        }
    }

    /// Render the fuzzy finder
    pub fn render(&mut self, f: &mut Frame, anchor_area: Rect) {
        // Calculate popup size (max 80x20, min 20x5)
        let popup_width = anchor_area.width.min(80).max(20);
        let popup_height = anchor_area.height.min(20).max(10);

        // Anchor popup so its bottom aligns with top of input box
        let x = anchor_area.x;
        let y = anchor_area.y.saturating_sub(popup_height).saturating_sub(1);

        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Clear the popup area
        f.render_widget(Clear, popup_area);

        // Split popup area into query input and results list
        let popup_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // TextArea with border
                Constraint::Min(0),    // Results list
            ])
            .split(popup_area);

        // Render the search input
        let mut textarea_with_border = self.textarea.clone();
        textarea_with_border.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        f.render_widget(&textarea_with_border, popup_chunks[0]);

        // Create the results list block
        let list_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Cyan));

        // Create list items from results
        let items: Vec<ListItem> = self
            .results
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let style = if i == self.selected {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default()
                };
                ListItem::new(path.as_str()).style(style)
            })
            .collect();

        // Create and render the list widget
        let list = List::new(items)
            .block(list_block)
            .highlight_style(Style::default().bg(Color::DarkGray));

        f.render_stateful_widget(list, popup_chunks[1], &mut self.list_state);
    }
}
