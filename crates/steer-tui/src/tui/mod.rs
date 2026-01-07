//! TUI module for the steer CLI
//!
//! This module implements the terminal user interface using ratatui.

use std::collections::{HashSet, VecDeque};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crate::tui::update::UpdateStatus;

use crate::error::{Error, Result};
use crate::tui::commands::registry::CommandRegistry;
use crate::tui::model::{ChatItem, NoticeLevel, TuiCommandResponse};
use crate::tui::theme::Theme;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, EventStream, KeyEventKind, MouseEvent};
use ratatui::{Frame, Terminal};
use steer_core::app::conversation::{AssistantContent, Message, MessageData};

use steer_grpc::AgentClient;
use steer_grpc::client_api::{ClientEvent, ModelId, OpId};

use crate::tui::events::processor::PendingToolApproval;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::tui::auth_controller::AuthController;
use crate::tui::events::pipeline::EventPipeline;
use crate::tui::events::processors::message::MessageEventProcessor;
use crate::tui::events::processors::processing_state::ProcessingStateProcessor;
use crate::tui::events::processors::system::SystemEventProcessor;
use crate::tui::events::processors::tool::ToolEventProcessor;
use crate::tui::state::RemoteProviderRegistry;
use crate::tui::state::SetupState;
use crate::tui::state::{ChatStore, ToolCallRegistry};

use crate::tui::chat_viewport::ChatViewport;
use crate::tui::terminal::{SetupGuard, cleanup};
use crate::tui::ui_layout::UiLayout;
use crate::tui::widgets::InputPanel;

pub mod commands;
pub mod custom_commands;
pub mod model;
pub mod state;
pub mod terminal;
pub mod theme;
pub mod widgets;

mod auth_controller;
mod chat_viewport;
pub mod core_commands;
mod events;
mod handlers;
mod ui_layout;
mod update;

#[cfg(test)]
mod test_utils;

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
    client: AgentClient,
    /// Are we currently processing a request?
    is_processing: bool,
    /// Progress message to show while processing
    progress_message: Option<String>,
    /// Animation frame for spinner
    spinner_state: usize,
    current_tool_approval: Option<PendingToolApproval>,
    /// Current model in use
    current_model: ModelId,
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
    in_flight_operations: HashSet<OpId>,
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
    /// Update checker status
    update_status: UpdateStatus,
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
        client: AgentClient,
        current_model: ModelId,

        session_id: String,
        theme: Option<Theme>,
    ) -> Result<Self> {
        // Set up terminal and ensure cleanup on early error
        let mut guard = SetupGuard::new();

        let mut stdout = io::stdout();
        terminal::setup(&mut stdout)?;

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

        let tui = Self {
            terminal,
            terminal_size,
            input_mode,
            input_panel_state: crate::tui::widgets::input_panel::InputPanelState::new(
                session_id.clone(),
            ),
            editing_message_id: None,
            client,
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
            update_status: UpdateStatus::Checking,
        };

        // Disarm guard; Tui instance will handle cleanup
        guard.disarm();

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

    fn push_tui_response(&mut self, command: String, response: TuiCommandResponse) {
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

    async fn start_new_session(&mut self) -> Result<()> {
        use std::collections::HashMap;
        use steer_core::session::state::{SessionConfig, SessionToolConfig, WorkspaceConfig};

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: HashMap::new(),
        };

        let new_session_id = self
            .client
            .create_session(session_config)
            .await
            .map_err(|e| Error::Generic(format!("Failed to create new session: {e}")))?;

        self.session_id = new_session_id.clone();
        self.chat_store = ChatStore::new();
        self.tool_registry = ToolCallRegistry::new();
        self.chat_viewport = ChatViewport::new();
        self.in_flight_operations.clear();
        self.input_panel_state =
            crate::tui::widgets::input_panel::InputPanelState::new(new_session_id.clone());
        self.is_processing = false;
        self.progress_message = None;
        self.current_tool_approval = None;
        self.editing_message_id = None;

        self.push_notice(
            NoticeLevel::Info,
            format!("Started new session: {}", new_session_id),
        );

        self.load_file_cache().await;

        Ok(())
    }

    async fn load_file_cache(&mut self) {
        info!(target: "tui.file_cache", "Requesting workspace files for session {}", self.session_id);
        match self.client.list_workspace_files().await {
            Ok(files) => {
                self.input_panel_state.file_cache.update(files).await;
            }
            Err(e) => {
                warn!(target: "tui.file_cache", "Failed to request workspace files: {}", e);
            }
        }
    }

    pub async fn run(&mut self, event_rx: mpsc::Receiver<ClientEvent>) -> Result<()> {
        // Log the current state of messages
        info!(
            "Starting TUI run with {} messages in view model",
            self.chat_store.len()
        );

        // Load the initial file list
        self.load_file_cache().await;

        // Spawn update checker
        let (update_tx, update_rx) = mpsc::channel::<UpdateStatus>(1);
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        tokio::spawn(async move {
            let status = update::check_latest("BrendanGraham14", "steer", &current_version).await;
            let _ = update_tx.send(status).await;
        });

        let mut term_event_stream = EventStream::new();

        // Run the main event loop
        self.run_event_loop(event_rx, &mut term_event_stream, update_rx)
            .await
    }

    async fn run_event_loop(
        &mut self,
        mut event_rx: mpsc::Receiver<ClientEvent>,
        term_event_stream: &mut EventStream,
        mut update_rx: mpsc::Receiver<UpdateStatus>,
    ) -> Result<()> {
        let mut should_exit = false;
        let mut needs_redraw = true; // Force initial draw
        let mut last_spinner_char = String::new();
        let mut update_rx_closed = false;

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
                status = update_rx.recv(), if !update_rx_closed => {
                    match status {
                        Some(status) => {
                            self.update_status = status;
                            needs_redraw = true;
                        }
                        None => {
                            // Channel closed; stop polling this branch to avoid busy looping
                            update_rx_closed = true;
                        }
                    }
                }
                event_res = term_event_stream.next() => {
                    match event_res {
                        Some(Ok(evt)) => match evt {
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
                        },
                        Some(Err(e)) => {
                            if e.kind() == io::ErrorKind::Interrupted {
                                debug!(target: "tui.input", "Ignoring interrupted syscall");
                            } else {
                                error!(target: "tui.run", "Fatal input error: {}. Exiting.", e);
                                should_exit = true;
                            }
                        }
                        None => {
                            // Input stream ended, request exit
                            should_exit = true;
                        }
                    }
                }
                client_event_opt = event_rx.recv() => {
                    match client_event_opt {
                        Some(client_event) => {
                            self.handle_client_event(client_event).await;
                            needs_redraw = true;
                        }
                        None => {
                            should_exit = true;
                        }
                    }
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
                    crate::tui::state::SetupStep::Authentication(provider_id) => {
                        AuthenticationWidget::render(
                            f.area(),
                            f.buffer_mut(),
                            setup_state,
                            provider_id.clone(),
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
            let current_tool_call = self.current_tool_approval.as_ref().map(|(_, tc)| tc);
            let current_model_owned = self.current_model.clone();

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
                current_tool_call,
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
                current_tool_call,
                is_processing,
                spinner_state,
                &self.theme,
            );
            f.render_stateful_widget(input_panel, layout.input_area, &mut self.input_panel_state);

            let update_badge = match &self.update_status {
                UpdateStatus::Available(info) => {
                    crate::tui::widgets::status_bar::UpdateBadge::Available {
                        latest: &info.latest,
                    }
                }
                _ => crate::tui::widgets::status_bar::UpdateBadge::None,
            };
            layout.render_status_bar(f, &current_model_owned, &self.theme, update_badge);

            // Get fuzzy finder results before the render call
            let fuzzy_finder_data = if input_mode == InputMode::FuzzyFinder {
                let results = self.input_panel_state.fuzzy_finder.results().to_vec();
                let selected = self.input_panel_state.fuzzy_finder.selected_index();
                let input_height = self.input_panel_state.required_height(
                    current_tool_call,
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
        results: &[crate::tui::widgets::fuzzy_finder::PickerItem],
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
                .map(|(i, item)| {
                    let is_selected = selected_index == i;
                    let style = if is_selected {
                        theme.style(theme::Component::PopupSelection)
                    } else {
                        Style::default()
                    };
                    ListItem::new(item.label.as_str()).style(style)
                })
                .collect(),
            crate::tui::widgets::fuzzy_finder::FuzzyFinderMode::Commands => {
                results
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

                        // Get command info to include description
                        let label = &item.label;
                        if let Some(cmd_info) = command_registry.get(label.as_str()) {
                            let line = format!("/{:<12} {}", cmd_info.name, cmd_info.description);
                            ListItem::new(line).style(style)
                        } else {
                            ListItem::new(format!("/{label}")).style(style)
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
                    ListItem::new(item.label.as_str()).style(style)
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

    async fn handle_client_event(&mut self, event: ClientEvent) {
        let mut messages_updated = false;

        match &event {
            ClientEvent::WorkspaceChanged => {
                self.load_file_cache().await;
            }
            ClientEvent::WorkspaceFiles { files } => {
                info!(target: "tui.handle_client_event", "Received workspace files event with {} files", files.len());
                self.input_panel_state
                    .file_cache
                    .update(files.clone())
                    .await;
            }
            _ => {}
        }

        let mut ctx = crate::tui::events::processor::ProcessingContext {
            chat_store: &mut self.chat_store,
            chat_list_state: self.chat_viewport.state_mut(),
            tool_registry: &mut self.tool_registry,
            client: &self.client,
            is_processing: &mut self.is_processing,
            progress_message: &mut self.progress_message,
            spinner_state: &mut self.spinner_state,
            current_tool_approval: &mut self.current_tool_approval,
            current_model: &mut self.current_model,
            messages_updated: &mut messages_updated,
            in_flight_operations: &mut self.in_flight_operations,
        };

        if let Err(e) = self.event_pipeline.process_event(event, &mut ctx).await {
            tracing::error!(target: "tui.handle_client_event", "Event processing failed: {}", e);
        }

        if self.current_tool_approval.is_some() && self.input_mode != InputMode::AwaitingApproval {
            self.switch_mode(InputMode::AwaitingApproval);
        } else if self.current_tool_approval.is_none()
            && self.input_mode == InputMode::AwaitingApproval
        {
            self.restore_previous_mode();
        }

        if messages_updated {
            if self.chat_viewport.state_mut().is_at_bottom() {
                self.chat_viewport.state_mut().scroll_to_bottom();
            }
        }
    }

    async fn send_message(&mut self, content: String) -> Result<()> {
        if content.starts_with('/') {
            return self.handle_slash_command(content).await;
        }

        if let Some(message_id_to_edit) = self.editing_message_id.take() {
            if let Err(e) = self
                .client
                .edit_message(message_id_to_edit, content, self.current_model.clone())
                .await
            {
                self.push_notice(NoticeLevel::Error, format!("Cannot edit message: {e}"));
            }
        } else {
            if let Err(e) = self
                .client
                .send_message(content, self.current_model.clone())
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
                                self.client
                                    .send_message(prompt, self.current_model.clone())
                                    .await?
                            }
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
                        self.input_panel_state.file_cache.clear().await;
                        info!(target: "tui.slash_command", "Cleared file cache, will reload on next access");
                        self.load_file_cache().await;
                        self.push_tui_response(
                            TuiCommandType::ReloadFiles.command_name(),
                            TuiCommandResponse::Text(
                                "File cache cleared. Files will be reloaded on next access."
                                    .to_string(),
                            ),
                        );
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
                                        TuiCommandResponse::Theme { name: name.clone() },
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
                            self.push_tui_response(
                                TuiCommandType::Theme.command_name(),
                                TuiCommandResponse::ListThemes(themes),
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

                        self.push_tui_response(
                            TuiCommandType::Help.command_name(),
                            TuiCommandResponse::Text(help_text),
                        );
                    }
                    TuiCommand::Auth => {
                        // Launch auth setup
                        // Initialize auth setup state
                        // Fetch providers and their auth status from server
                        let providers = self.client.list_providers().await.map_err(|e| {
                            crate::error::Error::Generic(format!(
                                "Failed to list providers from server: {e}"
                            ))
                        })?;
                        let statuses =
                            self.client
                                .get_provider_auth_status(None)
                                .await
                                .map_err(|e| {
                                    crate::error::Error::Generic(format!(
                                        "Failed to get provider auth status: {e}"
                                    ))
                                })?;

                        // Build provider registry view from remote providers
                        let mut provider_status = std::collections::HashMap::new();

                        use steer_grpc::proto::provider_auth_status::Status;
                        let mut status_map = std::collections::HashMap::new();
                        for s in statuses {
                            status_map.insert(s.provider_id.clone(), s.status);
                        }

                        // Convert remote providers into a minimal registry-like view for TUI
                        let registry =
                            std::sync::Arc::new(RemoteProviderRegistry::from_proto(providers));

                        for p in registry.all() {
                            let status = match status_map.get(&p.id).copied() {
                                Some(v) if v == Status::AuthStatusOauth as i32 => {
                                    crate::tui::state::AuthStatus::OAuthConfigured
                                }
                                Some(v) if v == Status::AuthStatusApiKey as i32 => {
                                    crate::tui::state::AuthStatus::ApiKeySet
                                }
                                _ => crate::tui::state::AuthStatus::NotConfigured,
                            };
                            provider_status.insert(
                                steer_core::config::provider::ProviderId(p.id.clone()),
                                status,
                            );
                        }

                        // Enter setup mode, skipping welcome page
                        self.setup_state =
                            Some(crate::tui::state::SetupState::new_for_auth_command(
                                registry,
                                provider_status,
                            ));
                        // Enter setup mode directly without pushing to the mode stack so that
                        // it can’t be accidentally popped by a later `restore_previous_mode`.
                        self.set_mode(InputMode::Setup);
                        // Clear the mode stack to avoid returning to a pre-setup mode.
                        self.mode_stack.clear();

                        self.push_tui_response(
                            TuiCommandType::Auth.to_string(),
                            TuiCommandResponse::Text(
                                "Entering authentication setup mode...".to_string(),
                            ),
                        );
                    }
                    TuiCommand::EditingMode(ref mode_name) => {
                        let response = match mode_name.as_deref() {
                            None => {
                                // Show current mode
                                let mode_str = self.preferences.ui.editing_mode.to_string();
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

                        self.push_tui_response(
                            tui_cmd.as_command_str(),
                            TuiCommandResponse::Text(response),
                        );
                    }
                    TuiCommand::Mcp => {
                        let servers = self.client.get_mcp_servers().await?;
                        self.push_tui_response(
                            tui_cmd.as_command_str(),
                            TuiCommandResponse::ListMcpServers(servers),
                        );
                    }
                    TuiCommand::Custom(custom_cmd) => match custom_cmd {
                        crate::tui::custom_commands::CustomCommand::Prompt { prompt, .. } => {
                            self.client
                                .send_message(prompt, self.current_model.clone())
                                .await?;
                        }
                    },
                    TuiCommand::New => {
                        self.start_new_session().await?;
                    }
                }
            }
            TuiAppCommand::Core(core_cmd) => {
                match core_cmd {
                    crate::tui::core_commands::CoreCommandType::Compact => {
                        if let Err(e) = self.client.compact_session().await {
                            self.push_notice(NoticeLevel::Error, format!("Compact failed: {e}"));
                        }
                    }
                    crate::tui::core_commands::CoreCommandType::Model { target } => {
                        if let Some(model_name) = target {
                            match self.client.resolve_model(&model_name).await {
                                Ok(model_id) => {
                                    self.current_model = model_id;
                                    self.push_notice(
                                        NoticeLevel::Info,
                                        format!(
                                            "Model set to: {}/{}",
                                            self.current_model.0.storage_key(),
                                            self.current_model.1
                                        ),
                                    );
                                }
                                Err(e) => {
                                    self.push_notice(
                                        NoticeLevel::Error,
                                        format!("Failed to resolve model: {e}"),
                                    );
                                }
                            }
                        } else {
                            self.push_notice(
                                NoticeLevel::Info,
                                format!(
                                    "Current model: {}/{}",
                                    self.current_model.0.storage_key(),
                                    self.current_model.1
                                ),
                            );
                        }
                    }
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
            .populate_edit_selection(self.chat_store.iter_items());

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

impl Drop for Tui {
    fn drop(&mut self) {
        // Use the same backend writer for reliable cleanup; idempotent via TERMINAL_STATE
        crate::tui::terminal::cleanup_with_writer(self.terminal.backend_mut());
    }
}

/// Helper to wrap terminal cleanup in panic handler
pub fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        cleanup();
        // Print panic info to stderr after restoring terminal state
        eprintln!("Application panicked:");
        eprintln!("{panic_info}");
    }));
}

/// High-level entry point for running the TUI
pub async fn run_tui(
    client: steer_grpc::AgentClient,
    session_id: Option<String>,
    model: steer_core::config::model::ModelId,
    directory: Option<std::path::PathBuf>,
    system_prompt: Option<String>,
    theme_name: Option<String>,
    force_setup: bool,
) -> Result<()> {
    use std::collections::HashMap;
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

    let (session_id, messages) = if let Some(session_id) = session_id {
        let (messages, _approved_tools) = client
            .get_conversation(&session_id)
            .await
            .map_err(Box::new)?;
        info!(
            "Loaded session: {} with {} messages",
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
        session_config.metadata.insert(
            "initial_model".to_string(),
            format!("{}/{}", model.0.storage_key(), model.1),
        );

        let session_id = client
            .create_session(session_config)
            .await
            .map_err(Box::new)?;
        (session_id, vec![])
    };

    client.subscribe_session_events().await.map_err(Box::new)?;
    let event_rx = client.subscribe_client_events().await;
    let mut tui = Tui::new(client, model.clone(), session_id.clone(), theme.clone()).await?;

    // Ensure terminal cleanup even if we error before entering the event loop
    struct TuiCleanupGuard;
    impl Drop for TuiCleanupGuard {
        fn drop(&mut self) {
            cleanup();
        }
    }
    let _cleanup_guard = TuiCleanupGuard;

    if !messages.is_empty() {
        tui.restore_messages(messages.clone());
    }

    // Query server for providers' auth status to decide if we should launch setup
    let statuses = tui
        .client
        .get_provider_auth_status(None)
        .await
        .map_err(|e| Error::Generic(format!("Failed to get provider auth status: {e}")))?;

    use steer_grpc::proto::provider_auth_status::Status as AuthStatusProto;
    let has_any_auth = statuses.iter().any(|s| {
        s.status == AuthStatusProto::AuthStatusOauth as i32
            || s.status == AuthStatusProto::AuthStatusApiKey as i32
    });

    let should_run_setup = force_setup
        || (!steer_core::preferences::Preferences::config_path()
            .map(|p| p.exists())
            .unwrap_or(false)
            && !has_any_auth);

    // Initialize setup state if first run or forced
    if should_run_setup {
        // Build registry for TUI sorting/labels from remote
        let providers =
            tui.client.list_providers().await.map_err(|e| {
                Error::Generic(format!("Failed to list providers from server: {e}"))
            })?;
        let registry = std::sync::Arc::new(RemoteProviderRegistry::from_proto(providers));

        // Map statuses by id for quick lookup
        let mut status_map = std::collections::HashMap::new();
        for s in statuses {
            status_map.insert(s.provider_id.clone(), s.status);
        }

        let mut provider_status = std::collections::HashMap::new();
        use steer_grpc::proto::provider_auth_status::Status as AuthStatusProto;
        for p in registry.all() {
            let status = match status_map.get(&p.id).copied() {
                Some(v) if v == AuthStatusProto::AuthStatusOauth as i32 => {
                    crate::tui::state::AuthStatus::OAuthConfigured
                }
                Some(v) if v == AuthStatusProto::AuthStatusApiKey as i32 => {
                    crate::tui::state::AuthStatus::ApiKeySet
                }
                _ => crate::tui::state::AuthStatus::NotConfigured,
            };
            provider_status.insert(
                steer_core::config::provider::ProviderId(p.id.clone()),
                status,
            );
        }

        tui.setup_state = Some(crate::tui::state::SetupState::new(
            registry,
            provider_status,
        ));
        tui.input_mode = InputMode::Setup;
    }

    // Run the TUI
    tui.run(event_rx).await
}

/// Run TUI in authentication setup mode
/// This is now just a convenience function that launches regular TUI with setup mode forced
pub async fn run_tui_auth_setup(
    client: steer_grpc::AgentClient,
    session_id: Option<String>,
    model: Option<ModelId>,
    session_db: Option<PathBuf>,
    theme_name: Option<String>,
) -> Result<()> {
    // Just delegate to regular run_tui - it will check for auth providers
    // and enter setup mode automatically if needed
    run_tui(
        client,
        session_id,
        model.unwrap_or(steer_core::config::model::builtin::claude_3_7_sonnet_20250219()),
        session_db,
        None, // system_prompt
        theme_name,
        true, // force_setup = true for auth setup
    )
    .await
}

#[cfg(test)]
mod tests {
    use crate::tui::test_utils::local_client_and_server;

    use super::*;

    use serde_json::json;

    use steer_core::app::conversation::{AssistantContent, Message, MessageData};
    use tempfile::tempdir;

    /// RAII guard to ensure terminal state is restored after a test, even on panic.
    struct TerminalCleanupGuard;

    impl Drop for TerminalCleanupGuard {
        fn drop(&mut self) {
            cleanup();
        }
    }

    #[tokio::test]
    #[ignore = "Requires TTY - run with `cargo test -- --ignored` in a terminal"]
    async fn test_restore_messages_preserves_tool_call_params() {
        let _guard = TerminalCleanupGuard;
        // Create a TUI instance for testing
        let path = tempdir().unwrap().path().to_path_buf();
        let (client, _server_handle) = local_client_and_server(Some(path)).await;
        let model = steer_core::config::model::builtin::claude_3_5_sonnet_20241022();
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(client, model, session_id, None).await.unwrap();

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
        let path = tempdir().unwrap().path().to_path_buf();
        let (client, _server_handle) = local_client_and_server(Some(path)).await;
        let model = steer_core::config::model::builtin::claude_3_5_sonnet_20241022();
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(client, model, session_id, None).await.unwrap();

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
