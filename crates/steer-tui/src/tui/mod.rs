//! TUI module for the steer CLI
//!
//! This module implements the terminal user interface using ratatui.

use std::collections::{HashSet, VecDeque};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::tui::commands::registry::CommandRegistry;
use crate::tui::model::{ChatItem, NoticeLevel};
use crate::tui::theme::Theme;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind, MouseEvent,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::SetTitle,
};
use ratatui::{Frame, Terminal};
use steer_core::api::Model;
use steer_core::app::conversation::{AssistantContent, Message, MessageData};
use steer_core::app::io::{AppCommandSink, AppEventSource};
use steer_core::app::{AppCommand, AppEvent};
use steer_core::config::LlmConfigProvider;
use steer_tools::schema::ToolCall;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::tui::auth_controller::AuthController;
use crate::tui::events::pipeline::EventPipeline;
use crate::tui::events::processors::message::MessageEventProcessor;
use crate::tui::events::processors::processing_state::ProcessingStateProcessor;
use crate::tui::events::processors::system::SystemEventProcessor;
use crate::tui::events::processors::tool::ToolEventProcessor;
use crate::tui::state::SetupState;
use crate::tui::state::{ChatStore, ToolCallRegistry};

use crate::tui::chat_viewport::ChatViewport;
use crate::tui::ui_layout::UiLayout;
use crate::tui::widgets::InputPanel;

pub mod commands;
pub mod custom_commands;
pub mod model;
pub mod state;
pub mod theme;
pub mod widgets;

mod auth_controller;
mod chat_viewport;
mod events;
mod handlers;
mod ui_layout;

/// How often to update the spinner animation (when processing)
const SPINNER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

/// Input modes for the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Simple mode - default non-modal editing
    Simple,
    /// Vim normal mode
    VimNormal,
    /// Vim insert mode
    VimInsert,
    /// Bash command mode - executing shell commands
    BashCommand,
    /// Awaiting tool approval
    AwaitingApproval,
    /// Confirm exit dialog
    ConfirmExit,
    /// Edit message selection mode with fuzzy filtering
    EditMessageSelection,
    /// Fuzzy finder mode for file selection
    FuzzyFinder,
    /// Setup mode - first run experience
    Setup,
}

/// Vim operator types
#[derive(Debug, Clone, Copy, PartialEq)]
enum VimOperator {
    Delete,
    Change,
    Yank,
}

/// State for tracking vim key sequences
#[derive(Debug, Default)]
struct VimState {
    /// Pending operator (d, c, y)
    pending_operator: Option<VimOperator>,
    /// Waiting for second 'g' in gg
    pending_g: bool,
    /// In replace mode (after 'r')
    replace_mode: bool,
    /// In visual mode
    visual_mode: bool,
}

/// Main TUI application state
pub struct Tui {
    /// Terminal instance
    terminal: Terminal<CrosstermBackend<Stdout>>,
    terminal_size: (u16, u16),
    /// Current input mode
    input_mode: InputMode,
    /// State for the input panel widget
    input_panel_state: crate::tui::widgets::input_panel::InputPanelState,
    /// The ID of the message being edited (if any)
    editing_message_id: Option<String>,
    /// Handle to send commands to the app
    command_sink: Arc<dyn AppCommandSink>,
    /// Are we currently processing a request?
    is_processing: bool,
    /// Progress message to show while processing
    progress_message: Option<String>,
    /// Animation frame for spinner
    spinner_state: usize,
    /// Current tool approval request
    current_tool_approval: Option<ToolCall>,
    /// Current model in use
    current_model: Model,
    /// Event processing pipeline
    event_pipeline: EventPipeline,
    /// Chat data store
    chat_store: ChatStore,
    /// Tool call registry
    tool_registry: ToolCallRegistry,
    /// Chat viewport for efficient rendering
    chat_viewport: ChatViewport,
    /// Session ID
    session_id: String,
    /// Current theme
    theme: Theme,
    /// Setup state for first-run experience
    setup_state: Option<SetupState>,
    /// Authentication controller (if active)
    auth_controller: Option<AuthController>,
    /// Track in-flight operations (operation_id -> chat_store_index)
    in_flight_operations: HashSet<uuid::Uuid>,
    /// Command registry for slash commands
    command_registry: CommandRegistry,
    /// User preferences
    preferences: steer_core::preferences::Preferences,
    /// Double-tap tracker for key sequences
    double_tap_tracker: crate::tui::state::DoubleTapTracker,
    /// Vim mode state
    vim_state: VimState,
    /// Stack to track previous modes (for returning after fuzzy finder, etc.)
    mode_stack: VecDeque<InputMode>,
    /// Last known revision of ChatStore for dirty tracking
    last_revision: u64,
}

const MAX_MODE_DEPTH: usize = 8;

impl Tui {
    /// Push current mode onto stack before switching
    fn push_mode(&mut self) {
        if self.mode_stack.len() == MAX_MODE_DEPTH {
            self.mode_stack.pop_front(); // drop oldest
        }
        self.mode_stack.push_back(self.input_mode);
    }

    /// Pop and restore previous mode
    fn pop_mode(&mut self) -> Option<InputMode> {
        self.mode_stack.pop_back()
    }

    /// Switch to a new mode, automatically managing the mode stack
    pub fn switch_mode(&mut self, new_mode: InputMode) {
        if self.input_mode != new_mode {
            debug!(
                "Switching mode from {:?} to {:?}",
                self.input_mode, new_mode
            );
            self.push_mode();
            self.input_mode = new_mode;
        }
    }

    /// Switch mode without pushing to stack (for direct transitions like vim normal->insert)
    pub fn set_mode(&mut self, new_mode: InputMode) {
        debug!("Setting mode from {:?} to {:?}", self.input_mode, new_mode);
        self.input_mode = new_mode;
    }

    /// Restore previous mode from stack (or default if empty)
    pub fn restore_previous_mode(&mut self) {
        self.input_mode = self.pop_mode().unwrap_or_else(|| self.default_input_mode());
    }

    /// Get the default input mode based on editing preferences
    fn default_input_mode(&self) -> InputMode {
        match self.preferences.ui.editing_mode {
            steer_core::preferences::EditingMode::Simple => InputMode::Simple,
            steer_core::preferences::EditingMode::Vim => InputMode::VimNormal,
        }
    }

    /// Check if current mode accepts text input
    fn is_text_input_mode(&self) -> bool {
        matches!(
            self.input_mode,
            InputMode::Simple
                | InputMode::VimInsert
                | InputMode::BashCommand
                | InputMode::Setup
                | InputMode::FuzzyFinder
        )
    }
    /// Create a new TUI instance
    pub async fn new(
        command_sink: Arc<dyn AppCommandSink>,
        current_model: Model,
        session_id: String,
        theme: Option<Theme>,
    ) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                ratatui::crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            ),
            EnableMouseCapture,
            SetTitle("Steer")
        )?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        let terminal_size = terminal
            .size()
            .map(|s| (s.width, s.height))
            .unwrap_or((80, 24));

        // Load preferences
        let preferences = steer_core::preferences::Preferences::load()
            .map_err(crate::error::Error::Core)
            .unwrap_or_default();

        // Determine initial input mode based on editing mode preference
        let input_mode = match preferences.ui.editing_mode {
            steer_core::preferences::EditingMode::Simple => InputMode::Simple,
            steer_core::preferences::EditingMode::Vim => InputMode::VimNormal,
        };

        // Create TUI with restored messages
        let mut tui = Self {
            terminal,
            terminal_size,
            input_mode,
            input_panel_state: crate::tui::widgets::input_panel::InputPanelState::new(
                session_id.clone(),
            ),
            editing_message_id: None,
            command_sink,
            is_processing: false,
            progress_message: None,
            spinner_state: 0,
            current_tool_approval: None,
            current_model,
            event_pipeline: Self::create_event_pipeline(),
            chat_store: ChatStore::new(),
            tool_registry: ToolCallRegistry::new(),
            chat_viewport: ChatViewport::new(),
            session_id,
            theme: theme.unwrap_or_default(),
            setup_state: None,
            auth_controller: None,
            in_flight_operations: HashSet::new(),
            command_registry: CommandRegistry::new(),
            preferences,
            double_tap_tracker: crate::tui::state::DoubleTapTracker::new(),
            vim_state: VimState::default(),
            mode_stack: VecDeque::new(),
            last_revision: 0,
        };

        Ok(tui)
    }

    /// Restore messages to the TUI, properly populating the tool registry
    fn restore_messages(&mut self, messages: Vec<Message>) {
        let message_count = messages.len();
        info!("Starting to restore {} messages to TUI", message_count);

        // Debug: log all Tool messages to check their IDs
        for message in &messages {
            if let steer_core::app::MessageData::Tool { tool_use_id, .. } = &message.data {
                debug!(
                    target: "tui.restore",
                    "Found Tool message with tool_use_id={}",
                    tool_use_id
                );
            }
        }

        self.chat_store.ingest_messages(&messages);

        // The rest of the tool registry population code remains the same
        // Extract tool calls from assistant messages
        for message in &messages {
            if let steer_core::app::MessageData::Assistant { content, .. } = &message.data {
                debug!(
                    target: "tui.restore",
                    "Processing Assistant message id={}",
                    message.id()
                );
                for block in content {
                    if let AssistantContent::ToolCall { tool_call } = block {
                        debug!(
                            target: "tui.restore",
                            "Found ToolCall in Assistant message: id={}, name={}, params={}",
                            tool_call.id, tool_call.name, tool_call.parameters
                        );

                        // Register the tool call
                        self.tool_registry.register_call(tool_call.clone());
                    }
                }
            }
        }

        // Map tool results to their calls
        for message in &messages {
            if let steer_core::app::MessageData::Tool { tool_use_id, .. } = &message.data {
                debug!(
                    target: "tui.restore",
                    "Updating registry with Tool result for id={}",
                    tool_use_id
                );
                // Tool results are already handled by event processors
            }
        }

        debug!(
            target: "tui.restore",
            "Tool registry state after restoration: {} calls registered",
            self.tool_registry.metrics().completed_count
        );
        info!("Successfully restored {} messages to TUI", message_count);
    }

    /// Helper to push a system notice to the chat store
    fn push_notice(&mut self, level: crate::tui::model::NoticeLevel, text: String) {
        use crate::tui::model::{ChatItem, ChatItemData, generate_row_id};
        self.chat_store.push(ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::SystemNotice {
                id: generate_row_id(),
                level,
                text,
                ts: time::OffsetDateTime::now_utc(),
            },
        });
    }

    /// Helper to push a TUI command response to the chat store
    fn push_tui_response(&mut self, command: String, response: String) {
        use crate::tui::model::{ChatItem, ChatItemData, generate_row_id};
        self.chat_store.push(ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::TuiCommandResponse {
                id: generate_row_id(),
                command,
                response,
                ts: time::OffsetDateTime::now_utc(),
            },
        });
    }

    /// Load file list into cache
    async fn load_file_cache(&mut self) {
        // Request workspace files from the server
        info!(target: "tui.file_cache", "Requesting workspace files for session {}", self.session_id);
        if let Err(e) = self
            .command_sink
            .send_command(AppCommand::RequestWorkspaceFiles)
            .await
        {
            warn!(target: "tui.file_cache", "Failed to request workspace files: {}", e);
        }
    }

    pub fn cleanup_terminal(&mut self) -> Result<()> {
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
            DisableMouseCapture
        )?;
        disable_raw_mode()?;
        Ok(())
    }

    pub async fn run(&mut self, mut event_rx: mpsc::Receiver<AppEvent>) -> Result<()> {
        // Log the current state of messages
        info!(
            "Starting TUI run with {} messages in view model",
            self.chat_store.len()
        );

        // Load the initial file list
        self.load_file_cache().await;

        let (term_event_tx, mut term_event_rx) = mpsc::channel::<Result<Event>>(1);
        let input_handle: JoinHandle<()> = tokio::spawn(async move {
            loop {
                // Non-blocking poll
                if event::poll(Duration::ZERO).unwrap_or(false) {
                    match event::read() {
                        Ok(evt) => {
                            if term_event_tx.send(Ok(evt)).await.is_err() {
                                break; // Receiver dropped
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                            // This is a non-fatal interrupted syscall, common on some
                            // systems. We just ignore it and continue polling.
                            debug!(target: "tui.input", "Ignoring interrupted syscall");
                            continue;
                        }
                        Err(e) => {
                            // A real I/O error occurred. Send it to the main loop
                            // to handle, and then stop polling.
                            warn!(target: "tui.input", "Input error: {}", e);
                            if term_event_tx.send(Err(Error::from(e))).await.is_err() {
                                break; // Receiver already dropped
                            }
                            break;
                        }
                    }
                } else {
                    // Async sleep that CAN be interrupted by abort
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        });

        let mut should_exit = false;
        let mut needs_redraw = true; // Force initial draw
        let mut last_spinner_char = String::new();

        // Create a tick interval for spinner updates
        let mut tick = tokio::time::interval(SPINNER_UPDATE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        while !should_exit {
            // Determine if we need to redraw
            if needs_redraw {
                self.draw()?;
                needs_redraw = false;
            }

            tokio::select! {
                Some(event_res) = term_event_rx.recv() => {
                    match event_res {
                        Ok(evt) => {
                            match evt {
                                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                                    match self.handle_key_event(key_event).await {
                                        Ok(exit) => {
                                            if exit {
                                                should_exit = true;
                                            }
                                        }
                                        Err(e) => {
                                            // Display error as a system notice
                                            use crate::tui::model::{ChatItem, ChatItemData, NoticeLevel, generate_row_id};
                                            self.chat_store.push(ChatItem {
                                                parent_chat_item_id: None,
                                                data: ChatItemData::SystemNotice {
                                                    id: generate_row_id(),
                                                    level: NoticeLevel::Error,
                                                    text: e.to_string(),
                                                    ts: time::OffsetDateTime::now_utc(),
                                                },
                                            });
                                        }
                                    }
                                    needs_redraw = true;
                                }
                                Event::Mouse(mouse_event) => {
                                    if self.handle_mouse_event(mouse_event)? {
                                        needs_redraw = true;
                                    }
                                }
                                Event::Resize(width, height) => {
                                    self.terminal_size = (width, height);
                                    // Terminal was resized, force redraw
                                    needs_redraw = true;
                                }
                                Event::Paste(data) => {
                                    // Handle paste in modes that accept text input
                                    if self.is_text_input_mode() {
                                        if self.input_mode == InputMode::Setup {
                                            // Handle paste in setup mode
                                            if let Some(setup_state) = &mut self.setup_state {
                                                match &setup_state.current_step {
                                                    crate::tui::state::SetupStep::Authentication(_) => {
                                                        if setup_state.oauth_state.is_some() {
                                                            // Pasting OAuth callback code
                                                            setup_state.oauth_callback_input.push_str(&data);
                                                        } else {
                                                            // Pasting API key
                                                            setup_state.api_key_input.push_str(&data);
                                                        }
                                                        debug!(target:"tui.run", "Pasted {} chars in Setup mode", data.len());
                                                        needs_redraw = true;
                                                    }
                                                    _ => {
                                                        // Other setup steps don't accept paste
                                                    }
                                                }
                                            }
                                        } else {
                                            let normalized_data =
                                                data.replace("\r\n", "\n").replace('\r', "\n");
                                            self.input_panel_state.insert_str(&normalized_data);
                                            debug!(target:"tui.run", "Pasted {} chars in {:?} mode", normalized_data.len(), self.input_mode);
                                            needs_redraw = true;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            error!(target: "tui.run", "Fatal input error: {}. Exiting.", e);
                            should_exit = true;
                        }
                    }
                }
                Some(app_event) = event_rx.recv() => {
                    self.handle_app_event(app_event).await;
                    needs_redraw = true;
                }
                _ = tick.tick() => {
                    // Check if we should animate the spinner
                    let has_pending_tools = !self.tool_registry.pending_calls().is_empty()
                        || !self.tool_registry.active_calls().is_empty()
                        || self.chat_store.has_pending_tools();
                    let has_in_flight_operations = !self.in_flight_operations.is_empty();

                    if self.is_processing || has_pending_tools || has_in_flight_operations {
                        self.spinner_state = self.spinner_state.wrapping_add(1);
                        let ch = get_spinner_char(self.spinner_state);
                        if ch != last_spinner_char {
                            last_spinner_char = ch.to_string();
                            needs_redraw = true;
                        }
                    }
                }
            }
        }

        // Cleanup terminal on exit
        self.cleanup_terminal()?;
        input_handle.abort();
        Ok(())
    }

    /// Handle mouse events
    fn handle_mouse_event(&mut self, event: MouseEvent) -> Result<bool> {
        let needs_redraw = match event.kind {
            event::MouseEventKind::ScrollUp => {
                // In vim normal mode or simple mode (when not typing), allow scrolling
                if !self.is_text_input_mode()
                    || (self.input_mode == InputMode::Simple
                        && self.input_panel_state.content().is_empty())
                {
                    self.chat_viewport.state_mut().scroll_up(3);
                    true
                } else {
                    false
                }
            }
            event::MouseEventKind::ScrollDown => {
                // In vim normal mode or simple mode (when not typing), allow scrolling
                if !self.is_text_input_mode()
                    || (self.input_mode == InputMode::Simple
                        && self.input_panel_state.content().is_empty())
                {
                    self.chat_viewport.state_mut().scroll_down(3);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };

        Ok(needs_redraw)
    }

    /// Draw the UI
    fn draw(&mut self) -> Result<()> {
        self.terminal.draw(|f| {
            // Check if we're in setup mode
            if let Some(setup_state) = &self.setup_state {
                use crate::tui::widgets::setup::{
                    authentication::AuthenticationWidget, completion::CompletionWidget,
                    provider_selection::ProviderSelectionWidget, welcome::WelcomeWidget,
                };

                match &setup_state.current_step {
                    crate::tui::state::SetupStep::Welcome => {
                        WelcomeWidget::render(f.area(), f.buffer_mut(), &self.theme);
                    }
                    crate::tui::state::SetupStep::ProviderSelection => {
                        ProviderSelectionWidget::render(
                            f.area(),
                            f.buffer_mut(),
                            setup_state,
                            &self.theme,
                        );
                    }
                    crate::tui::state::SetupStep::Authentication(provider) => {
                        AuthenticationWidget::render(
                            f.area(),
                            f.buffer_mut(),
                            setup_state,
                            *provider,
                            &self.theme,
                        );
                    }
                    crate::tui::state::SetupStep::Completion => {
                        CompletionWidget::render(
                            f.area(),
                            f.buffer_mut(),
                            setup_state,
                            &self.theme,
                        );
                    }
                }
                return;
            }

            let input_mode = self.input_mode;
            let is_processing = self.is_processing;
            let spinner_state = self.spinner_state;
            let current_tool_approval = self.current_tool_approval.as_ref();
            let current_model_owned = self.current_model;

            // Check if ChatStore has changed and trigger rebuild if needed
            let current_revision = self.chat_store.revision();
            if current_revision != self.last_revision {
                self.chat_viewport.mark_dirty();
                self.last_revision = current_revision;
            }

            // Get chat items from the chat store
            let chat_items: Vec<&ChatItem> = self.chat_store.as_items();

            let terminal_size = f.area();

            let input_area_height = self.input_panel_state.required_height(
                current_tool_approval,
                terminal_size.width,
                terminal_size.height,
            );

            let layout = UiLayout::compute(terminal_size, input_area_height, &self.theme);
            layout.prepare_background(f, &self.theme);

            self.chat_viewport.rebuild(
                &chat_items,
                layout.chat_area.width,
                self.chat_viewport.state().view_mode,
                &self.theme,
                &self.chat_store,
            );

            let hovered_id = self
                .input_panel_state
                .get_hovered_id()
                .map(|s| s.to_string());

            self.chat_viewport.render(
                f,
                layout.chat_area,
                spinner_state,
                hovered_id.as_deref(),
                &self.theme,
            );

            let input_panel = InputPanel::new(
                input_mode,
                current_tool_approval,
                is_processing,
                spinner_state,
                &self.theme,
            );
            f.render_stateful_widget(input_panel, layout.input_area, &mut self.input_panel_state);

            // Render status bar
            layout.render_status_bar(f, &current_model_owned, &self.theme);

            // Get fuzzy finder results before the render call
            let fuzzy_finder_data = if input_mode == InputMode::FuzzyFinder {
                let results = self.input_panel_state.fuzzy_finder.results().to_vec();
                let selected = self.input_panel_state.fuzzy_finder.selected_index();
                let input_height = self.input_panel_state.required_height(
                    current_tool_approval,
                    terminal_size.width,
                    10,
                );
                let mode = self.input_panel_state.fuzzy_finder.mode();
                Some((results, selected, input_height, mode))
            } else {
                None
            };

            // Render fuzzy finder overlay when active
            if let Some((results, selected_index, input_height, mode)) = fuzzy_finder_data {
                Self::render_fuzzy_finder_overlay_static(
                    f,
                    &results,
                    selected_index,
                    input_height,
                    mode,
                    &self.theme,
                    &self.command_registry,
                );
            }
        })?;
        Ok(())
    }

    /// Render fuzzy finder overlay above the input panel
    fn render_fuzzy_finder_overlay_static(
        f: &mut Frame,
        results: &[String],
        selected_index: usize,
        input_panel_height: u16,
        mode: crate::tui::widgets::fuzzy_finder::FuzzyFinderMode,
        theme: &Theme,
        command_registry: &CommandRegistry,
    ) {
        use ratatui::layout::Rect;
        use ratatui::style::Style;
        use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

        // imports already handled above

        if results.is_empty() {
            return; // Nothing to show
        }

        // Get the terminal area and calculate input panel position
        let total_area = f.area();

        // Calculate where the input panel would be
        let input_panel_y = total_area.height.saturating_sub(input_panel_height + 1); // +1 for status bar

        // Calculate overlay height (max 10 results)
        let overlay_height = results.len().min(10) as u16 + 2; // +2 for borders

        // Position overlay just above the input panel
        let overlay_y = input_panel_y.saturating_sub(overlay_height);
        let overlay_area = Rect {
            x: total_area.x,
            y: overlay_y,
            width: total_area.width,
            height: overlay_height,
        };

        // Clear the area first
        f.render_widget(Clear, overlay_area);

        // Create list items with selection highlighting
        // Reverse the order so best match (index 0) is at the bottom
        let items: Vec<ListItem> = match mode {
            crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Files => results
                .iter()
                .enumerate()
                .rev()
                .map(|(i, path)| {
                    let is_selected = selected_index == i;
                    let style = if is_selected {
                        theme.style(theme::Component::PopupSelection)
                    } else {
                        Style::default()
                    };
                    ListItem::new(path.as_str()).style(style)
                })
                .collect(),
            crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Commands => {
                results
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(i, cmd_name)| {
                        let is_selected = selected_index == i;
                        let style = if is_selected {
                            theme.style(theme::Component::PopupSelection)
                        } else {
                            Style::default()
                        };

                        // Get command info to include description
                        if let Some(cmd_info) = command_registry.get(cmd_name.as_str()) {
                            let line = format!("/{:<12} {}", cmd_info.name, cmd_info.description);
                            ListItem::new(line).style(style)
                        } else {
                            ListItem::new(format!("/{cmd_name}")).style(style)
                        }
                    })
                    .collect()
            }
            crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Models
            | crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Themes => results
                .iter()
                .enumerate()
                .rev()
                .map(|(i, item)| {
                    let is_selected = selected_index == i;
                    let style = if is_selected {
                        theme.style(theme::Component::PopupSelection)
                    } else {
                        Style::default()
                    };
                    ListItem::new(item.as_str()).style(style)
                })
                .collect(),
        };

        // Create the list widget
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.style(theme::Component::PopupBorder))
            .title(match mode {
                crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Files => " Files ",
                crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Commands => " Commands ",
                crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Models => " Select Model ",
                crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Themes => " Select Theme ",
            });

        let list = List::new(items)
            .block(list_block)
            .highlight_style(theme.style(theme::Component::PopupSelection));

        // Create list state with reversed selection
        let mut list_state = ListState::default();
        let reversed_selection = results
            .len()
            .saturating_sub(1)
            .saturating_sub(selected_index);
        list_state.select(Some(reversed_selection));

        f.render_stateful_widget(list, overlay_area, &mut list_state);
    }

    /// Create the event processing pipeline
    fn create_event_pipeline() -> EventPipeline {
        EventPipeline::new()
            .add_processor(Box::new(ProcessingStateProcessor::new()))
            .add_processor(Box::new(MessageEventProcessor::new()))
            .add_processor(Box::new(ToolEventProcessor::new()))
            .add_processor(Box::new(SystemEventProcessor::new()))
    }

    async fn handle_app_event(&mut self, event: AppEvent) {
        let mut messages_updated = false;

        // Handle workspace events before processing through pipeline
        match &event {
            AppEvent::WorkspaceChanged => {
                self.load_file_cache().await;
            }
            AppEvent::WorkspaceFiles { files } => {
                // Update file cache with the new file list
                info!(target: "tui.handle_app_event", "Received workspace files event with {} files", files.len());
                self.input_panel_state
                    .file_cache
                    .update(files.clone())
                    .await;
            }
            _ => {}
        }

        // Create processing context
        let mut ctx = crate::tui::events::processor::ProcessingContext {
            chat_store: &mut self.chat_store,
            chat_list_state: self.chat_viewport.state_mut(),
            tool_registry: &mut self.tool_registry,
            command_sink: &self.command_sink,
            is_processing: &mut self.is_processing,
            progress_message: &mut self.progress_message,
            spinner_state: &mut self.spinner_state,
            current_tool_approval: &mut self.current_tool_approval,
            current_model: &mut self.current_model,
            messages_updated: &mut messages_updated,
            in_flight_operations: &mut self.in_flight_operations,
        };

        // Process the event through the pipeline
        if let Err(e) = self.event_pipeline.process_event(event, &mut ctx).await {
            tracing::error!(target: "tui.handle_app_event", "Event processing failed: {}", e);
        }

        // Sync doesn't need to happen anymore since we don't track threads

        // Handle special input mode changes for tool approval
        if self.current_tool_approval.is_some() && self.input_mode != InputMode::AwaitingApproval {
            self.switch_mode(InputMode::AwaitingApproval);
        } else if self.current_tool_approval.is_none()
            && self.input_mode == InputMode::AwaitingApproval
        {
            self.restore_previous_mode();
        }

        // Auto-scroll if messages were added
        if messages_updated {
            // Clear cache for any updated messages
            // Scroll to bottom if we were already at the bottom
            if self.chat_viewport.state_mut().is_at_bottom() {
                self.chat_viewport.state_mut().scroll_to_bottom();
            }
        }
    }

    async fn send_message(&mut self, content: String) -> Result<()> {
        // Handle slash commands
        if content.starts_with('/') {
            return self.handle_slash_command(content).await;
        }

        // Check if we're editing a message
        if let Some(message_id_to_edit) = self.editing_message_id.take() {
            // Send edit command which creates a new branch
            if let Err(e) = self
                .command_sink
                .send_command(AppCommand::EditMessage {
                    message_id: message_id_to_edit,
                    new_content: content,
                })
                .await
            {
                self.push_notice(NoticeLevel::Error, format!("Cannot edit message: {e}"));
            }
        } else {
            // Send regular message
            if let Err(e) = self
                .command_sink
                .send_command(AppCommand::ProcessUserInput(content))
                .await
            {
                self.push_notice(NoticeLevel::Error, format!("Cannot send message: {e}"));
            }
        }
        Ok(())
    }

    async fn handle_slash_command(&mut self, command_input: String) -> Result<()> {
        use crate::tui::commands::{AppCommand as TuiAppCommand, TuiCommand, TuiCommandType};
        use crate::tui::model::NoticeLevel;

        // First check if it's a custom command in the registry
        let cmd_name = command_input
            .trim()
            .strip_prefix('/')
            .unwrap_or(command_input.trim());

        if let Some(cmd_info) = self.command_registry.get(cmd_name) {
            if let crate::tui::commands::registry::CommandScope::Custom(custom_cmd) =
                &cmd_info.scope
            {
                // Create a TuiCommand::Custom and process it
                let app_cmd = TuiAppCommand::Tui(TuiCommand::Custom(custom_cmd.clone()));
                // Process through the normal flow
                match app_cmd {
                    TuiAppCommand::Tui(TuiCommand::Custom(custom_cmd)) => {
                        // Handle custom command based on its type
                        match custom_cmd {
                            crate::tui::custom_commands::CustomCommand::Prompt {
                                prompt, ..
                            } => {
                                // Forward prompt directly as user input to avoid recursive slash handling
                                self.command_sink
                                    .send_command(AppCommand::ProcessUserInput(prompt))
                                    .await?;
                            } // Future custom command types can be handled here
                        }
                    }
                    _ => unreachable!(),
                }
                return Ok(());
            }
        }

        // Otherwise try to parse as built-in command
        let app_cmd = match TuiAppCommand::parse(&command_input) {
            Ok(cmd) => cmd,
            Err(e) => {
                // Add error notice to chat
                self.push_notice(NoticeLevel::Error, e.to_string());
                return Ok(());
            }
        };

        // Handle the command based on its type
        match app_cmd {
            TuiAppCommand::Tui(tui_cmd) => {
                // Handle TUI-specific commands
                match tui_cmd {
                    TuiCommand::ReloadFiles => {
                        // Clear the file cache to force a refresh
                        self.input_panel_state.file_cache.clear().await;
                        info!(target: "tui.slash_command", "Cleared file cache, will reload on next access");
                        // Request workspace files again
                        if let Err(e) = self
                            .command_sink
                            .send_command(AppCommand::RequestWorkspaceFiles)
                            .await
                        {
                            self.push_notice(
                                NoticeLevel::Error,
                                format!("Cannot reload files: {e}"),
                            );
                        } else {
                            self.push_tui_response(
                                TuiCommandType::ReloadFiles.command_name(),
                                "File cache cleared. Files will be reloaded on next access."
                                    .to_string(),
                            );
                        }
                    }
                    TuiCommand::Theme(theme_name) => {
                        if let Some(name) = theme_name {
                            // Load the specified theme
                            let loader = theme::ThemeLoader::new();
                            match loader.load_theme(&name) {
                                Ok(new_theme) => {
                                    self.theme = new_theme;
                                    self.push_tui_response(
                                        TuiCommandType::Theme.command_name(),
                                        format!("Theme changed to '{name}'"),
                                    );
                                }
                                Err(e) => {
                                    self.push_notice(
                                        NoticeLevel::Error,
                                        format!("Failed to load theme '{name}': {e}"),
                                    );
                                }
                            }
                        } else {
                            // List available themes
                            let loader = theme::ThemeLoader::new();
                            let themes = loader.list_themes();
                            let theme_list = if themes.is_empty() {
                                "No themes found.".to_string()
                            } else {
                                format!("Available themes:\n{}", themes.join("\n"))
                            };
                            self.push_tui_response(
                                TuiCommandType::Theme.command_name(),
                                theme_list,
                            );
                        }
                    }
                    TuiCommand::Help(command_name) => {
                        // Build and show help text
                        let help_text = if let Some(cmd_name) = command_name {
                            // Show help for specific command
                            if let Some(cmd_info) = self.command_registry.get(&cmd_name) {
                                format!(
                                    "Command: {}\n\nDescription: {}\n\nUsage: {}",
                                    cmd_info.name, cmd_info.description, cmd_info.usage
                                )
                            } else {
                                format!("Unknown command: {cmd_name}")
                            }
                        } else {
                            // Show general help with all commands
                            let mut help_lines = vec!["Available commands:".to_string()];
                            for cmd_info in self.command_registry.all_commands() {
                                help_lines.push(format!(
                                    "  {:<20} - {}",
                                    cmd_info.usage, cmd_info.description
                                ));
                            }
                            help_lines.join("\n")
                        };

                        self.push_tui_response(TuiCommandType::Help.command_name(), help_text);
                    }
                    TuiCommand::Auth => {
                        // Launch auth setup
                        // Initialize auth setup state
                        let auth_storage =
                            steer_core::auth::DefaultAuthStorage::new().map_err(|e| {
                                crate::error::Error::Generic(format!(
                                    "Failed to create auth storage: {e}"
                                ))
                            })?;
                        let auth_providers = LlmConfigProvider::new(Arc::new(auth_storage))
                            .available_providers()
                            .await?;

                        let mut provider_status = std::collections::HashMap::new();
                        for provider in [
                            steer_core::api::ProviderKind::Anthropic,
                            steer_core::api::ProviderKind::OpenAI,
                            steer_core::api::ProviderKind::Google,
                            steer_core::api::ProviderKind::XAI,
                        ] {
                            let status = if auth_providers.contains(&provider) {
                                crate::tui::state::AuthStatus::ApiKeySet
                            } else {
                                crate::tui::state::AuthStatus::NotConfigured
                            };
                            provider_status.insert(provider, status);
                        }

                        // Enter setup mode, skipping welcome page
                        self.setup_state = Some(
                            crate::tui::state::SetupState::new_for_auth_command(provider_status),
                        );
                        // Enter setup mode directly without pushing to the mode stack so that
                        // it can’t be accidentally popped by a later `restore_previous_mode`.
                        self.set_mode(InputMode::Setup);
                        // Clear the mode stack to avoid returning to a pre-setup mode.
                        self.mode_stack.clear();

                        self.push_tui_response(
                            TuiCommandType::Auth.to_string(),
                            "Entering authentication setup mode...".to_string(),
                        );
                    }
                    TuiCommand::EditingMode(ref mode_name) => {
                        let response = match mode_name.as_deref() {
                            None => {
                                // Show current mode
                                let mode_str = match self.preferences.ui.editing_mode {
                                    steer_core::preferences::EditingMode::Simple => "simple",
                                    steer_core::preferences::EditingMode::Vim => "vim",
                                };
                                format!("Current editing mode: {mode_str}")
                            }
                            Some("simple") => {
                                self.preferences.ui.editing_mode =
                                    steer_core::preferences::EditingMode::Simple;
                                self.set_mode(InputMode::Simple);
                                self.preferences.save().map_err(crate::error::Error::Core)?;
                                "Switched to Simple mode".to_string()
                            }
                            Some("vim") => {
                                self.preferences.ui.editing_mode =
                                    steer_core::preferences::EditingMode::Vim;
                                self.set_mode(InputMode::VimNormal);
                                self.preferences.save().map_err(crate::error::Error::Core)?;
                                "Switched to Vim mode (Normal)".to_string()
                            }
                            Some(mode) => {
                                format!("Unknown mode: '{mode}'. Use 'simple' or 'vim'")
                            }
                        };

                        self.push_tui_response(tui_cmd.as_command_str(), response);
                    }
                    TuiCommand::Custom(custom_cmd) => {
                        // Handle custom command based on its type
                        match custom_cmd {
                            crate::tui::custom_commands::CustomCommand::Prompt {
                                prompt, ..
                            } => {
                                // Forward prompt directly as user input to avoid recursive slash handling
                                self.command_sink
                                    .send_command(AppCommand::ProcessUserInput(prompt))
                                    .await?;
                            } // Future custom command types can be handled here
                        }
                    }
                }
            }
            TuiAppCommand::Core(core_cmd) => {
                // Pass core commands through to the backend
                if let Err(e) = self
                    .command_sink
                    .send_command(AppCommand::ExecuteCommand(core_cmd))
                    .await
                {
                    self.push_notice(NoticeLevel::Error, e.to_string());
                }
            }
        }

        Ok(())
    }

    /// Enter edit mode for a specific message
    fn enter_edit_mode(&mut self, message_id: &str) {
        // Find the message in the store
        if let Some(item) = self.chat_store.get_by_id(&message_id.to_string()) {
            if let crate::tui::model::ChatItemData::Message(message) = &item.data {
                if let MessageData::User { content, .. } = &message.data {
                    // Extract text content from user blocks
                    let text = content
                        .iter()
                        .filter_map(|block| match block {
                            steer_core::app::conversation::UserContent::Text { text } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    // Set up textarea with the message content
                    self.input_panel_state
                        .set_content_from_lines(text.lines().collect::<Vec<_>>());
                    // Switch to appropriate mode based on editing preference
                    self.input_mode = match self.preferences.ui.editing_mode {
                        steer_core::preferences::EditingMode::Simple => InputMode::Simple,
                        steer_core::preferences::EditingMode::Vim => InputMode::VimInsert,
                    };

                    // Store the message ID we're editing
                    self.editing_message_id = Some(message_id.to_string());
                }
            }
        }
    }

    /// Scroll chat list to show a specific message
    fn scroll_to_message_id(&mut self, message_id: &str) {
        // Find the index of the message in the chat store
        let mut target_index = None;
        for (idx, item) in self.chat_store.items().enumerate() {
            if let crate::tui::model::ChatItemData::Message(message) = &item.data {
                if message.id() == message_id {
                    target_index = Some(idx);
                    break;
                }
            }
        }

        if let Some(idx) = target_index {
            // Scroll to center the message if possible
            self.chat_viewport.state_mut().scroll_to_item(idx);
        }
    }

    /// Enter edit message selection mode
    fn enter_edit_selection_mode(&mut self) {
        self.switch_mode(InputMode::EditMessageSelection);

        // Populate the edit selection messages in the input panel state
        self.input_panel_state
            .populate_edit_selection(self.chat_store.iter_items().map(|item| &item.data));

        // Scroll to the hovered message if there is one
        if let Some(id) = self.input_panel_state.get_hovered_id() {
            let id = id.to_string();
            self.scroll_to_message_id(&id);
        }
    }
}

/// Helper function to get spinner character
fn get_spinner_char(state: usize) -> &'static str {
    const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    SPINNER_CHARS[state % SPINNER_CHARS.len()]
}

/// Free function for best-effort terminal cleanup (raw mode, alt screen, mouse, etc.)
pub fn cleanup_terminal() {
    use ratatui::crossterm::{
        event::{DisableBracketedPaste, DisableMouseCapture, PopKeyboardEnhancementFlags},
        execute,
        terminal::{LeaveAlternateScreen, disable_raw_mode},
    };
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture
    );
}

/// Helper to wrap terminal cleanup in panic handler
pub fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        cleanup_terminal();
        // Print panic info to stderr after restoring terminal state
        eprintln!("Application panicked:");
        eprintln!("{panic_info}");
    }));
}

/// High-level entry point for running the TUI
pub async fn run_tui(
    client: std::sync::Arc<steer_grpc::GrpcClientAdapter>,
    session_id: Option<String>,
    model: steer_core::api::Model,
    directory: Option<std::path::PathBuf>,
    system_prompt: Option<String>,
    theme_name: Option<String>,
    force_setup: bool,
) -> Result<()> {
    use std::collections::HashMap;
    use steer_core::app::io::{AppCommandSink, AppEventSource};
    use steer_core::session::{SessionConfig, SessionToolConfig};

    // Load theme - use catppuccin-mocha as default if none specified
    let loader = theme::ThemeLoader::new();
    let theme = if let Some(theme_name) = theme_name {
        // Check if theme_name is an absolute path
        let path = std::path::Path::new(&theme_name);
        let theme_result = if path.is_absolute() || path.exists() {
            // Load from specific path
            loader.load_theme_from_path(path)
        } else {
            // Load by name from search paths
            loader.load_theme(&theme_name)
        };

        match theme_result {
            Ok(theme) => {
                info!("Loaded theme: {}", theme_name);
                Some(theme)
            }
            Err(e) => {
                warn!(
                    "Failed to load theme '{}': {}. Using default theme.",
                    theme_name, e
                );
                // Fall back to catppuccin-mocha
                loader.load_theme("catppuccin-mocha").ok()
            }
        }
    } else {
        // No theme specified, use catppuccin-mocha as default
        match loader.load_theme("catppuccin-mocha") {
            Ok(theme) => {
                info!("Loaded default theme: catppuccin-mocha");
                Some(theme)
            }
            Err(e) => {
                warn!(
                    "Failed to load default theme 'catppuccin-mocha': {}. Using hardcoded default.",
                    e
                );
                None
            }
        }
    };

    // If session_id is provided, resume that session
    let (session_id, messages) = if let Some(session_id) = session_id {
        // Activate the existing session
        let (messages, _approved_tools) = client
            .activate_session(session_id.clone())
            .await
            .map_err(Box::new)?;
        info!(
            "Activated session: {} with {} messages",
            session_id,
            messages.len()
        );
        println!("Session ID: {session_id}");
        (session_id, messages)
    } else {
        // Create a new session
        let mut session_config = SessionConfig {
            workspace: if let Some(ref dir) = directory {
                steer_core::session::state::WorkspaceConfig::Local { path: dir.clone() }
            } else {
                steer_core::session::state::WorkspaceConfig::default()
            },
            tool_config: SessionToolConfig::default(),
            system_prompt,
            metadata: HashMap::new(),
        };

        // Add the initial model to session metadata
        session_config
            .metadata
            .insert("initial_model".to_string(), model.to_string());

        let session_id = client
            .create_session(session_config)
            .await
            .map_err(Box::new)?;
        (session_id, vec![])
    };

    client.start_streaming().await.map_err(Box::new)?;
    let event_rx = client.subscribe().await;
    let mut tui = Tui::new(
        client.clone() as std::sync::Arc<dyn AppCommandSink>,
        model,
        session_id,
        theme.clone(),
    )
    .await?;

    if !messages.is_empty() {
        tui.restore_messages(messages);
    }

    let auth_storage = steer_core::auth::DefaultAuthStorage::new()
        .map_err(|e| Error::Generic(format!("Failed to create auth storage: {e}")))?;
    let auth_providers = LlmConfigProvider::new(Arc::new(auth_storage))
        .available_providers()
        .await
        .map_err(|e| Error::Generic(format!("Failed to check auth: {e}")))?;

    let should_run_setup = force_setup
        || (!steer_core::preferences::Preferences::config_path()
            .map(|p| p.exists())
            .unwrap_or(false)
            && auth_providers.is_empty());

    // Initialize setup state if first run or forced
    if should_run_setup {
        let mut provider_status = std::collections::HashMap::new();
        for provider in [
            steer_core::api::ProviderKind::Anthropic,
            steer_core::api::ProviderKind::OpenAI,
            steer_core::api::ProviderKind::Google,
            steer_core::api::ProviderKind::XAI,
        ] {
            let status = if auth_providers.contains(&provider) {
                crate::tui::state::AuthStatus::ApiKeySet
            } else {
                crate::tui::state::AuthStatus::NotConfigured
            };
            provider_status.insert(provider, status);
        }

        tui.setup_state = Some(crate::tui::state::SetupState::new(provider_status));
        tui.input_mode = InputMode::Setup;
    }

    // Run the TUI
    tui.run(event_rx).await?;

    Ok(())
}

/// Run TUI in authentication setup mode
/// This is now just a convenience function that launches regular TUI with setup mode forced
pub async fn run_tui_auth_setup(
    client: std::sync::Arc<steer_grpc::GrpcClientAdapter>,
    session_id: Option<String>,
    model: Option<Model>,
    session_db: Option<PathBuf>,
    theme_name: Option<String>,
) -> Result<()> {
    // Just delegate to regular run_tui - it will check for auth providers
    // and enter setup mode automatically if needed
    run_tui(
        client,
        session_id,
        model.unwrap_or_default(),
        session_db,
        None, // system_prompt
        theme_name,
        true, // force_setup = true for auth setup
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use steer_core::app::AppCommand;
    use steer_core::app::AppEvent;
    use steer_core::app::conversation::{AssistantContent, Message, MessageData};
    use steer_core::app::io::{AppCommandSink, AppEventSource};
    use steer_core::error::Result;
    use tokio::sync::mpsc;

    /// RAII guard to ensure terminal state is restored after a test, even on panic.
    struct TerminalCleanupGuard;

    impl Drop for TerminalCleanupGuard {
        fn drop(&mut self) {
            cleanup_terminal();
        }
    }

    // Mock command sink for tests
    struct MockCommandSink;

    #[async_trait]
    impl AppCommandSink for MockCommandSink {
        async fn send_command(&self, _command: AppCommand) -> Result<()> {
            Ok(())
        }
    }

    struct MockEventSource;

    #[async_trait]
    impl AppEventSource for MockEventSource {
        async fn subscribe(&self) -> mpsc::Receiver<AppEvent> {
            let (_, rx) = mpsc::channel(10);
            rx
        }
    }

    #[tokio::test]
    #[ignore = "Requires TTY - run with `cargo test -- --ignored` in a terminal"]
    async fn test_restore_messages_preserves_tool_call_params() {
        let _guard = TerminalCleanupGuard;
        // Create a TUI instance for testing
        let command_sink = Arc::new(MockCommandSink) as Arc<dyn AppCommandSink>;
        let model = steer_core::api::Model::Claude3_5Sonnet20241022;
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(command_sink, model, session_id, None)
            .await
            .unwrap();

        // Build test messages: Assistant with ToolCall, then Tool result
        let tool_id = "test_tool_123".to_string();
        let tool_call = steer_tools::ToolCall {
            id: tool_id.clone(),
            name: "view".to_string(),
            parameters: json!({
                "file_path": "/test/file.rs",
                "offset": 10,
                "limit": 100
            }),
        };

        let assistant_msg = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: tool_call.clone(),
                }],
            },
            id: "msg_assistant".to_string(),
            timestamp: 1234567890,
            parent_message_id: None,
        };

        let tool_msg = Message {
            data: MessageData::Tool {
                tool_use_id: tool_id.clone(),
                result: steer_tools::ToolResult::FileContent(
                    steer_tools::result::FileContentResult {
                        file_path: "/test/file.rs".to_string(),
                        content: "file content here".to_string(),
                        line_count: 1,
                        truncated: false,
                    },
                ),
            },
            id: "msg_tool".to_string(),
            timestamp: 1234567891,
            parent_message_id: Some("msg_assistant".to_string()),
        };

        let messages = vec![assistant_msg, tool_msg];

        // Restore messages
        tui.restore_messages(messages);

        // Verify tool call was preserved in registry
        let stored_call = tui
            .tool_registry
            .get_tool_call(&tool_id)
            .expect("Tool call should be in registry");
        assert_eq!(stored_call.name, "view");
        assert_eq!(stored_call.parameters, tool_call.parameters);
    }

    #[tokio::test]
    #[ignore = "Requires TTY - run with `cargo test -- --ignored` in a terminal"]
    async fn test_restore_messages_handles_tool_result_before_assistant() {
        let _guard = TerminalCleanupGuard;
        // Test edge case where Tool result arrives before Assistant message
        let command_sink = Arc::new(MockCommandSink) as Arc<dyn AppCommandSink>;
        let model = steer_core::api::Model::Claude3_5Sonnet20241022;
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(command_sink, model, session_id, None)
            .await
            .unwrap();

        let tool_id = "test_tool_456".to_string();
        let real_params = json!({
            "file_path": "/another/file.rs"
        });

        let tool_call = steer_tools::ToolCall {
            id: tool_id.clone(),
            name: "view".to_string(),
            parameters: real_params.clone(),
        };

        // Tool result comes first (unusual but possible)
        let tool_msg = Message {
            data: MessageData::Tool {
                tool_use_id: tool_id.clone(),
                result: steer_tools::ToolResult::FileContent(
                    steer_tools::result::FileContentResult {
                        file_path: "/another/file.rs".to_string(),
                        content: "file content".to_string(),
                        line_count: 1,
                        truncated: false,
                    },
                ),
            },
            id: "msg_tool".to_string(),
            timestamp: 1234567890,
            parent_message_id: None,
        };

        let assistant_msg = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: tool_call.clone(),
                }],
            },
            id: "msg_456".to_string(),
            timestamp: 1234567891,
            parent_message_id: None,
        };

        let messages = vec![tool_msg, assistant_msg];

        tui.restore_messages(messages);

        // Should still have proper parameters
        let stored_call = tui
            .tool_registry
            .get_tool_call(&tool_id)
            .expect("Tool call should be in registry");
        assert_eq!(stored_call.parameters, real_params);
        assert_eq!(stored_call.name, "view");
    }
}
