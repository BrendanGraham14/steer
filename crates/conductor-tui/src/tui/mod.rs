//! TUI module for the conductor CLI
//!
//! This module implements the terminal user interface using ratatui.

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::tui::theme::Theme;
use conductor_core::api::Model;
use conductor_core::app::conversation::{AssistantContent, Message};
use conductor_core::app::io::{AppCommandSink, AppEventSource};
use conductor_core::app::{AppCommand, AppEvent};
use conductor_tools::schema::ToolCall;
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
use ratatui::layout::{Constraint, Direction, Layout};

use ratatui::widgets::Borders;
use ratatui::{Frame, Terminal};
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
use crate::tui::state::view_model::MessageViewModel;

use crate::tui::widgets::chat_list::{ChatList, ChatListState, ViewMode};
use crate::tui::widgets::{InputPanel, InputPanelState, StatusBar};

pub mod commands;
pub mod model;
pub mod state;
pub mod theme;
pub mod widgets;

mod auth_controller;
mod events;
mod handlers;

/// Popup state for model selection
#[derive(Debug, Clone, Default)]
struct PopupState {
    selected: Option<usize>,
}

impl PopupState {
    fn select(&mut self, index: Option<usize>) {
        self.selected = index;
    }

    fn selected(&self) -> Option<usize> {
        self.selected
    }

    fn next(&mut self, total: usize) {
        if total == 0 {
            return;
        }
        self.selected = Some(match self.selected {
            None => 0,
            Some(i) => (i + 1) % total,
        });
    }

    fn previous(&mut self) {
        if let Some(i) = self.selected {
            self.selected = Some(if i == 0 { 0 } else { i - 1 });
        }
    }
}

/// How often to update the spinner animation (when processing)
const SPINNER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

/// Input modes for the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode - navigation and commands
    Normal,
    /// Insert mode - typing messages
    Insert,
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
    /// Available models for selection
    models: Vec<Model>,
    /// Current model in use
    current_model: Model,
    /// Popup selection state (when showing model list)
    popup_state: PopupState,
    /// Event processing pipeline
    event_pipeline: EventPipeline,
    /// Message view model (data + ui state)
    view_model: MessageViewModel,
    /// Session ID
    session_id: String,
    /// Current theme
    theme: Theme,
    /// Setup state for first-run experience
    setup_state: Option<SetupState>,
    /// Authentication controller (if active)
    auth_controller: Option<AuthController>,
}

impl Tui {
    /// Create a new TUI instance
    pub async fn new(
        command_sink: Arc<dyn AppCommandSink>,
        _event_source: Arc<dyn AppEventSource>,
        current_model: Model,
        models: Vec<Model>,
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
            SetTitle("Conductor")
        )?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        let terminal_size = terminal
            .size()
            .map(|s| (s.width, s.height))
            .unwrap_or((80, 24));

        // Request current conversation
        let messages = Self::get_current_conversation(_event_source).await?;
        info!(
            "Received {} messages from current conversation",
            messages.len()
        );

        // Create TUI with restored messages
        let mut tui = Self {
            terminal,
            terminal_size,
            input_mode: InputMode::Normal,
            input_panel_state: crate::tui::widgets::input_panel::InputPanelState::new(
                session_id.clone(),
            ),
            editing_message_id: None,
            command_sink,
            is_processing: false,
            progress_message: None,
            spinner_state: 0,
            current_tool_approval: None,
            models,
            current_model,
            popup_state: PopupState::default(),
            event_pipeline: Self::create_event_pipeline(),
            view_model: MessageViewModel::new(),
            session_id,
            theme: theme.unwrap_or_default(),
            setup_state: None,
            auth_controller: None,
        };

        // Restore messages using the public method
        tui.restore_messages(messages);

        Ok(tui)
    }

    /// Restore messages to the TUI, properly populating the tool registry
    fn restore_messages(&mut self, messages: Vec<Message>) {
        let message_count = messages.len();
        info!("Starting to restore {} messages to TUI", message_count);

        // If we have messages, use the thread ID from the most recent one
        if let Some(last_msg) = messages.last() {
            self.view_model.current_thread = Some(*last_msg.thread_id());
            info!(
                "Setting current thread to: {:?}",
                self.view_model.current_thread
            );
        }

        // Debug: log all Tool messages to check their IDs
        for message in &messages {
            if let conductor_core::app::Message::Tool { tool_use_id, .. } = message {
                debug!(
                    target: "tui.restore",
                    "Found Tool message with tool_use_id={}",
                    tool_use_id
                );
            }
        }

        // First pass: populate tool registry with tool calls from assistant messages
        for message in &messages {
            if let conductor_core::app::Message::Assistant { content, .. } = message {
                for item in content {
                    if let AssistantContent::ToolCall { tool_call } = item {
                        debug!(
                            target: "tui.restore",
                            "Registering tool call: id={}, name={}, params={}",
                            tool_call.id,
                            tool_call.name,
                            tool_call.parameters
                        );
                        self.view_model.tool_registry.upsert_call(tool_call.clone());
                    }
                }
            }
        }

        // Debug dump the registry state after first pass
        self.view_model.tool_registry.debug_dump("After first pass");

        // Use the view model's add_messages method to add all messages at once
        self.view_model.add_messages(messages);

        info!(
            "Finished restoring messages. TUI now has {} messages",
            self.view_model.chat_store.len()
        );

        // Reset scroll to bottom after restoring messages
        self.view_model.chat_list_state.scroll_to_bottom();
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
            self.view_model.chat_store.len()
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
                                            use crate::tui::model::{ChatItem, NoticeLevel, generate_row_id};
                                            let notice = ChatItem::SystemNotice {
                                                id: generate_row_id(),
                                                level: NoticeLevel::Error,
                                                text: e.to_string(),
                                                ts: time::OffsetDateTime::now_utc(),
                                            };
                                            self.view_model.chat_store.push(notice);
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
                                    if matches!(
                                        self.input_mode,
                                        InputMode::Insert | InputMode::BashCommand | InputMode::Setup
                                    ) {
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
                    if self.is_processing {
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
                match self.input_mode {
                    InputMode::Normal => {
                        self.view_model.chat_list_state.scroll_up(3);
                        true
                    }
                    InputMode::Insert => {
                        // In insert mode, let textarea handle scrolling if needed
                        false
                    }
                    _ => false,
                }
            }
            event::MouseEventKind::ScrollDown => {
                match self.input_mode {
                    InputMode::Normal => {
                        self.view_model.chat_list_state.scroll_down(3);
                        true
                    }
                    InputMode::Insert => {
                        // In insert mode, let textarea handle scrolling if needed
                        false
                    }
                    _ => false,
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

            // Get chat items from the chat store
            let chat_items = self.view_model.chat_store.as_slice();

            // Get the hovered ID before the render call to avoid borrowing issues
            let hovered_id = self
                .input_panel_state
                .get_hovered_id()
                .map(|s| s.to_string());

            // Get fuzzy finder results before the render call
            let fuzzy_finder_data = if input_mode == InputMode::FuzzyFinder {
                let results = self.input_panel_state.fuzzy_finder.results().to_vec();
                let selected = self.input_panel_state.fuzzy_finder.selected_index();
                let input_height = self.input_panel_state.required_height(10);
                Some((results, selected, input_height))
            } else {
                None
            };

            if let Err(e) = Tui::render_ui_static(
                f,
                chat_items,
                &mut self.view_model.chat_list_state,
                input_mode,
                &current_model_owned,
                current_tool_approval,
                self.view_model.current_thread,
                is_processing,
                spinner_state,
                hovered_id.as_deref(),
                &mut self.input_panel_state,
                &self.theme,
            ) {
                error!(target:"tui.run.draw", "UI rendering failed: {}", e);
            }

            // Render fuzzy finder overlay when active
            if let Some((results, selected_index, input_height)) = fuzzy_finder_data {
                Self::render_fuzzy_finder_overlay_static(
                    f,
                    &results,
                    selected_index,
                    input_height,
                    &self.theme,
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
        theme: &Theme,
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
        let items: Vec<ListItem> = results
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
            .collect();

        // Create the list widget
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.style(theme::Component::PopupBorder))
            .title(" Files (best match at bottom) ");

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
            chat_store: &mut self.view_model.chat_store,
            chat_list_state: &mut self.view_model.chat_list_state,
            tool_registry: &mut self.view_model.tool_registry,
            command_sink: &self.command_sink,
            is_processing: &mut self.is_processing,
            progress_message: &mut self.progress_message,
            spinner_state: &mut self.spinner_state,
            current_tool_approval: &mut self.current_tool_approval,
            current_model: &mut self.current_model,
            messages_updated: &mut messages_updated,
            current_thread: self.view_model.current_thread,
        };

        // Process the event through the pipeline
        if let Err(e) = self.event_pipeline.process_event(event, &mut ctx).await {
            tracing::error!(target: "tui.handle_app_event", "Event processing failed: {}", e);
        }

        // Sync thread if we don't have one and messages were added
        if self.view_model.current_thread.is_none() && !self.view_model.chat_store.is_empty() {
            // Find first message with a real thread ID
            let thread_id = self
                .view_model
                .chat_store
                .messages()
                .into_iter()
                .next()
                .map(|row| *row.inner.thread_id());

            if let Some(thread_id) = thread_id {
                // Set as active thread
                self.view_model.set_thread(thread_id);
            }
        }

        // Handle special input mode changes for tool approval
        if self.current_tool_approval.is_some() && self.input_mode != InputMode::AwaitingApproval {
            self.input_mode = InputMode::AwaitingApproval;
        } else if self.current_tool_approval.is_none()
            && self.input_mode == InputMode::AwaitingApproval
        {
            self.input_mode = InputMode::Normal;
        }

        // Auto-scroll if messages were added
        if messages_updated {
            // Clear cache for any updated messages
            // Scroll to bottom if we were already at the bottom
            if self.view_model.chat_list_state.is_at_bottom() {
                self.view_model.chat_list_state.scroll_to_bottom();
            }
        }
    }

    async fn get_current_conversation(
        _event_source: Arc<dyn AppEventSource>,
    ) -> Result<Vec<Message>> {
        // For now, just return empty - conversation will be restored by the app
        Ok(Vec::new())
    }

    /// Static UI rendering function
    #[allow(clippy::too_many_arguments)]
    fn render_ui_static(
        f: &mut Frame,
        chat_items: &[crate::tui::model::ChatItem],
        chat_list_state: &mut ChatListState,
        input_mode: InputMode,
        current_model: &Model,
        current_approval: Option<&ToolCall>,
        _current_thread: Option<uuid::Uuid>,
        is_processing: bool,
        spinner_state: usize,
        edit_selection_hovered_id: Option<&str>,
        input_panel_state: &mut InputPanelState,
        theme: &Theme,
    ) -> Result<()> {
        let size = f.area();

        // Clear the entire terminal area with the theme's background color
        use ratatui::widgets::{Block, Clear};
        f.render_widget(Clear, size);

        // Apply background color if theme has one
        if let Some(bg_color) = theme.get_background_color() {
            let background_block =
                Block::default().style(ratatui::style::Style::default().bg(bg_color));
            f.render_widget(background_block, size);
        }

        // Calculate required height for input/approval area
        let input_area_height = if let Some(tool_call) = current_approval {
            // For approval mode: use the state's calculation
            InputPanelState::required_height_for_approval(
                tool_call,
                size.width,
                size.height.saturating_sub(4) / 2,
            )
        } else {
            // For input mode: use the state's calculation
            input_panel_state.required_height(10)
        };

        // Main layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),                    // Messages area (flexible)
                Constraint::Length(input_area_height), // Input area (dynamic)
                Constraint::Length(1),                 // Status bar
            ])
            .split(size);

        // Render messages
        let _message_block = Block::default()
            .borders(Borders::ALL)
            .title(" Messages ")
            .style(theme.style(theme::Component::ChatListBorder));

        // Use the ChatList widget as a stateful widget
        let chat_list =
            ChatList::new(chat_items, theme).hovered_message_id(edit_selection_hovered_id);
        f.render_stateful_widget(chat_list, chunks[0], chat_list_state);

        // Render input panel using stateful widget
        let input_panel = InputPanel::new(
            input_mode,
            current_approval,
            is_processing,
            spinner_state,
            theme,
        );
        f.render_stateful_widget(input_panel, chunks[1], input_panel_state);

        // Render status bar using the new widget
        let status_bar = StatusBar::new(current_model, theme);
        f.render_widget(status_bar, chunks[2]);

        Ok(())
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
                use crate::tui::model::{ChatItem, NoticeLevel, generate_row_id};
                let notice = ChatItem::SystemNotice {
                    id: generate_row_id(),
                    level: NoticeLevel::Error,
                    text: format!("Cannot edit message: {e}"),
                    ts: time::OffsetDateTime::now_utc(),
                };
                self.view_model.chat_store.push(notice);
            }
        } else {
            // Send regular message
            if let Err(e) = self
                .command_sink
                .send_command(AppCommand::ProcessUserInput(content))
                .await
            {
                use crate::tui::model::{ChatItem, NoticeLevel, generate_row_id};
                let notice = ChatItem::SystemNotice {
                    id: generate_row_id(),
                    level: NoticeLevel::Error,
                    text: format!("Cannot send message: {e}"),
                    ts: time::OffsetDateTime::now_utc(),
                };
                self.view_model.chat_store.push(notice);
            }
        }
        Ok(())
    }

    async fn handle_slash_command(&mut self, command: String) -> Result<()> {
        use crate::tui::commands::{AppCommand as TuiAppCommand, TuiCommand};
        use crate::tui::model::{ChatItem, NoticeLevel, generate_row_id};

        let app_cmd = match TuiAppCommand::parse(&command) {
            Ok(cmd) => cmd,
            Err(e) => {
                // Add error notice to chat
                let error_msg = e.to_string();
                let notice = ChatItem::SystemNotice {
                    id: generate_row_id(),
                    level: NoticeLevel::Error,
                    text: error_msg,
                    ts: time::OffsetDateTime::now_utc(),
                };
                self.view_model.chat_store.push(notice);
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
                            let notice = ChatItem::SystemNotice {
                                id: generate_row_id(),
                                level: NoticeLevel::Error,
                                text: format!("Cannot reload files: {e}"),
                                ts: time::OffsetDateTime::now_utc(),
                            };
                            self.view_model.chat_store.push(notice);
                        }
                    }
                    TuiCommand::Theme(theme_name) => {
                        if let Some(name) = theme_name {
                            // Load the specified theme
                            let loader = theme::ThemeLoader::new();
                            match loader.load_theme(&name) {
                                Ok(new_theme) => {
                                    self.theme = new_theme;
                                    let notice = ChatItem::SystemNotice {
                                        id: generate_row_id(),
                                        level: NoticeLevel::Info,
                                        text: format!("Loaded theme: {name}"),
                                        ts: time::OffsetDateTime::now_utc(),
                                    };
                                    self.view_model.chat_store.push(notice);
                                }
                                Err(e) => {
                                    let notice = ChatItem::SystemNotice {
                                        id: generate_row_id(),
                                        level: NoticeLevel::Error,
                                        text: format!("Failed to load theme '{name}': {e}"),
                                        ts: time::OffsetDateTime::now_utc(),
                                    };
                                    self.view_model.chat_store.push(notice);
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
                            let notice = ChatItem::SystemNotice {
                                id: generate_row_id(),
                                level: NoticeLevel::Info,
                                text: theme_list,
                                ts: time::OffsetDateTime::now_utc(),
                            };
                            self.view_model.chat_store.push(notice);
                        }
                    }
                    TuiCommand::Auth => {
                        // Launch auth setup
                        // Initialize auth setup state
                        let auth_storage = conductor_core::auth::DefaultAuthStorage::new()
                            .map_err(|e| {
                                crate::error::Error::Generic(format!(
                                    "Failed to create auth storage: {e}"
                                ))
                            })?;
                        let auth_providers =
                            conductor_core::auth::inspect::get_authenticated_providers(
                                &auth_storage,
                            )
                            .await
                            .map_err(|e| {
                                crate::error::Error::Generic(format!("Failed to check auth: {e}"))
                            })?;

                        let mut provider_status = std::collections::HashMap::new();
                        for provider in [
                            conductor_core::api::ProviderKind::Anthropic,
                            conductor_core::api::ProviderKind::OpenAI,
                            conductor_core::api::ProviderKind::Google,
                            conductor_core::api::ProviderKind::Grok,
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
                        self.input_mode = InputMode::Setup;

                        let notice = ChatItem::SystemNotice {
                            id: generate_row_id(),
                            level: NoticeLevel::Info,
                            text: "Entering authentication setup mode...".to_string(),
                            ts: time::OffsetDateTime::now_utc(),
                        };
                        self.view_model.chat_store.push(notice);
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
                    let notice = ChatItem::SystemNotice {
                        id: generate_row_id(),
                        level: NoticeLevel::Error,
                        text: e.to_string(),
                        ts: time::OffsetDateTime::now_utc(),
                    };
                    self.view_model.chat_store.push(notice);
                }
            }
        }

        Ok(())
    }

    /// Enter edit mode for a specific message
    fn enter_edit_mode(&mut self, message_id: &str) {
        // Find the message in the store
        if let Some(crate::tui::model::ChatItem::Message(row)) = self
            .view_model
            .chat_store
            .get_by_id(&message_id.to_string())
        {
            if let Message::User { content, .. } = &row.inner {
                // Extract text content from user blocks
                let text = content
                    .iter()
                    .filter_map(|block| match block {
                        conductor_core::app::conversation::UserContent::Text { text } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                // Set up textarea with the message content
                self.input_panel_state
                    .set_content_from_lines(text.lines().collect::<Vec<_>>());
                self.input_mode = InputMode::Insert;

                // Store the message ID we're editing
                self.editing_message_id = Some(message_id.to_string());
            }
        }
    }

    /// Scroll chat list to show a specific message
    fn scroll_to_message_id(&mut self, message_id: &str) {
        // Find the index of the message in the chat store
        let mut target_index = None;
        for (idx, item) in self.view_model.chat_store.as_slice().iter().enumerate() {
            if let crate::tui::model::ChatItem::Message(row) = item {
                if row.inner.id() == message_id {
                    target_index = Some(idx);
                    break;
                }
            }
        }

        if let Some(idx) = target_index {
            // Scroll to center the message if possible
            self.view_model.chat_list_state.scroll_to_item(idx);
        }
    }

    /// Enter edit message selection mode
    fn enter_edit_selection_mode(&mut self) {
        self.input_mode = InputMode::EditMessageSelection;

        // Populate the edit selection messages in the input panel state
        self.input_panel_state
            .populate_edit_selection(self.view_model.chat_store.as_slice());

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

// -------------------------------------------------------------------------------------------------
// Public API for backward compatibility with conductor-cli
// -------------------------------------------------------------------------------------------------

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
    client: std::sync::Arc<conductor_grpc::GrpcClientAdapter>,
    session_id: Option<String>,
    model: conductor_core::api::Model,
    directory: Option<std::path::PathBuf>,
    system_prompt: Option<String>,
    theme_name: Option<String>,
    force_setup: bool,
) -> Result<()> {
    use conductor_core::app::io::{AppCommandSink, AppEventSource};
    use conductor_core::session::{SessionConfig, SessionToolConfig};
    use std::collections::HashMap;

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
                conductor_core::session::state::WorkspaceConfig::Local { path: dir.clone() }
            } else {
                conductor_core::session::state::WorkspaceConfig::default()
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
        client.clone() as std::sync::Arc<dyn AppEventSource>,
        model,
        vec![model], // For now, just pass the single model
        session_id,
        theme.clone(),
    )
    .await?;

    if !messages.is_empty() {
        tui.restore_messages(messages);
    }

    let should_run_setup = force_setup
        || (!conductor_core::preferences::Preferences::config_path()
            .map(|p| p.exists())
            .unwrap_or(false)
            && conductor_core::auth::inspect::get_authenticated_providers(
                &conductor_core::auth::DefaultAuthStorage::new()
                    .map_err(|e| Error::Generic(format!("Failed to create auth storage: {e}")))?,
            )
            .await
            .unwrap_or_default()
            .is_empty());

    // Initialize setup state if first run or forced
    if should_run_setup {
        let auth_storage = conductor_core::auth::DefaultAuthStorage::new()
            .map_err(|e| Error::Generic(format!("Failed to create auth storage: {e}")))?;
        let auth_providers =
            conductor_core::auth::inspect::get_authenticated_providers(&auth_storage)
                .await
                .map_err(|e| Error::Generic(format!("Failed to check auth: {e}")))?;

        let mut provider_status = std::collections::HashMap::new();
        for provider in [
            conductor_core::api::ProviderKind::Anthropic,
            conductor_core::api::ProviderKind::OpenAI,
            conductor_core::api::ProviderKind::Google,
            conductor_core::api::ProviderKind::Grok,
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
    client: std::sync::Arc<conductor_grpc::GrpcClientAdapter>,
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
    use conductor_core::app::AppCommand;
    use conductor_core::app::AppEvent;
    use conductor_core::app::conversation::{AssistantContent, Message};
    use conductor_core::app::io::{AppCommandSink, AppEventSource};
    use conductor_core::error::Result;
    use serde_json::json;
    use std::sync::Arc;
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
        let event_source = Arc::new(MockEventSource) as Arc<dyn AppEventSource>;
        let model = conductor_core::api::Model::Claude3_5Sonnet20241022;
        let models = vec![model];
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(command_sink, event_source, model, models, session_id, None)
            .await
            .unwrap();

        // Build test messages: Assistant with ToolCall, then Tool result
        let tool_id = "test_tool_123".to_string();
        let tool_call = conductor_tools::ToolCall {
            id: tool_id.clone(),
            name: "view".to_string(),
            parameters: json!({
                "file_path": "/test/file.rs",
                "offset": 10,
                "limit": 100
            }),
        };

        let assistant_msg = Message::Assistant {
            id: "msg_assistant".to_string(),
            content: vec![AssistantContent::ToolCall {
                tool_call: tool_call.clone(),
            }],
            timestamp: 1234567890,
            thread_id: uuid::Uuid::new_v4(),
            parent_message_id: None,
        };

        let tool_msg = Message::Tool {
            id: "msg_tool".to_string(),
            tool_use_id: tool_id.clone(),
            result: conductor_tools::ToolResult::FileContent(
                conductor_tools::result::FileContentResult {
                    file_path: "/test/file.rs".to_string(),
                    content: "file content here".to_string(),
                    line_count: 1,
                    truncated: false,
                },
            ),
            timestamp: 1234567891,
            thread_id: *assistant_msg.thread_id(),
            parent_message_id: Some("msg_assistant".to_string()),
        };

        let messages = vec![assistant_msg, tool_msg];

        // Restore messages
        tui.restore_messages(messages);

        // Verify tool call was preserved in registry
        let stored_call = tui
            .view_model
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
        let event_source = Arc::new(MockEventSource) as Arc<dyn AppEventSource>;
        let model = conductor_core::api::Model::Claude3_5Sonnet20241022;
        let models = vec![model];
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(command_sink, event_source, model, models, session_id, None)
            .await
            .unwrap();

        let tool_id = "test_tool_456".to_string();
        let real_params = json!({
            "file_path": "/another/file.rs"
        });

        let tool_call = conductor_tools::ToolCall {
            id: tool_id.clone(),
            name: "view".to_string(),
            parameters: real_params.clone(),
        };

        // Tool result comes first (unusual but possible)
        let tool_msg = Message::Tool {
            id: "msg_tool".to_string(),
            tool_use_id: tool_id.clone(),
            result: conductor_tools::ToolResult::FileContent(
                conductor_tools::result::FileContentResult {
                    file_path: "/another/file.rs".to_string(),
                    content: "file content".to_string(),
                    line_count: 1,
                    truncated: false,
                },
            ),
            timestamp: 1234567890,
            thread_id: uuid::Uuid::new_v4(),
            parent_message_id: None,
        };

        let assistant_msg = Message::Assistant {
            id: "msg_456".to_string(),
            content: vec![AssistantContent::ToolCall {
                tool_call: tool_call.clone(),
            }],
            timestamp: 1234567891,
            thread_id: *tool_msg.thread_id(),
            parent_message_id: None,
        };

        let messages = vec![tool_msg, assistant_msg];

        tui.restore_messages(messages);

        // Should still have proper parameters
        let stored_call = tui
            .view_model
            .tool_registry
            .get_tool_call(&tool_id)
            .expect("Tool call should be in registry");
        assert_eq!(stored_call.parameters, real_params);
        assert_eq!(stored_call.name, "view");
    }
}
