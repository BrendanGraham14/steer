use ratatui::widgets::ListState;

/// Type of content being searched
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FuzzyFinderMode {
    /// Searching for files (triggered by @)
    Files,
    /// Searching for commands (triggered by /)
    Commands,
}

/// Result of fuzzy finder operations
pub enum FuzzyFinderResult {
    /// User wants to close the finder
    Close,
    /// User selected a file
    Select(String),
}

/// A fuzzy finder component for file selection
#[derive(Debug)]
pub struct FuzzyFinder {
    /// Whether the finder is currently active
    active: bool,
    /// Current search results
    results: Vec<String>,
    /// Currently selected result index
    selected: usize,
    /// List state for scrolling
    list_state: ListState,
    /// The byte position of the @ that triggered this fuzzy finder
    trigger_position: Option<usize>,
    /// The mode of the fuzzy finder (files or commands)
    mode: FuzzyFinderMode,
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
            results: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            trigger_position: None,
            mode: FuzzyFinderMode::Files,
        }
    }

    /// Check if the finder is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Activate the fuzzy finder with the position of the trigger character and mode
    pub fn activate(&mut self, trigger_position: usize, mode: FuzzyFinderMode) {
        self.active = true;
        self.selected = 0;
        self.results.clear();
        self.list_state = ListState::default();
        self.trigger_position = Some(trigger_position);
        self.mode = mode;
    }

    /// Deactivate the fuzzy finder
    pub fn deactivate(&mut self) {
        self.active = false;
        self.results.clear();
        self.selected = 0;
        self.list_state = ListState::default();
        self.trigger_position = None;
    }

    /// Get the trigger position (@ character position)
    pub fn trigger_position(&self) -> Option<usize> {
        self.trigger_position
    }

    /// Get the current selected index
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Get the current results
    pub fn results(&self) -> &[String] {
        &self.results
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

    /// Get the current mode
    pub fn mode(&self) -> FuzzyFinderMode {
        self.mode
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
            (KeyCode::Enter, _) | (KeyCode::Tab, _) => {
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
                // Move selection up (visually down in reversed list)
                if self.selected + 1 < self.results.len() {
                    self.selected += 1;
                    self.list_state.select(Some(self.selected));
                }
                None
            }
            (KeyCode::Down, _) => {
                // Move selection down (visually up in reversed list)
                if self.selected > 0 {
                    self.selected -= 1;
                    self.list_state.select(Some(self.selected));
                }
                None
            }
            _ => {
                // For other keys, return None to let the parent handle text input
                None
            }
        }
    }
}
