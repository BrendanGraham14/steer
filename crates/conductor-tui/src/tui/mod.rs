//! TUI module for the conductor CLI
//!
//! This module implements the terminal user interface using ratatui.

use std::io::{self, Stdout};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use conductor_core::api::Model;
use conductor_core::app::conversation::{AssistantContent, Message};
use conductor_core::app::io::{AppCommandSink, AppEventSource};
use conductor_core::app::{AppCommand, AppEvent};
use conductor_tools::schema::ToolCall;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEvent, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::SetTitle,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};
use tui_textarea::{Input, Key, TextArea};

use crate::tui::events::pipeline::EventPipeline;
use crate::tui::events::processors::message::MessageEventProcessor;
use crate::tui::events::processors::processing_state::ProcessingStateProcessor;
use crate::tui::events::processors::system::SystemEventProcessor;
use crate::tui::events::processors::tool::ToolEventProcessor;
use crate::tui::state::{ContentCache, MessageViewModel};
use crate::tui::widgets::cached_renderer::CachedContentRenderer;
use crate::tui::widgets::chat_list::{ChatList, ChatListState, ViewMode};
use crate::tui::widgets::popup_list::PopupList;

pub mod model;
pub mod state;
pub mod widgets;

mod events;

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
enum InputMode {
    /// Normal mode - navigation and commands
    Normal,
    /// Insert mode - typing messages
    Insert,
    /// Bash command mode - executing shell commands
    BashCommand,
    /// Awaiting tool approval
    AwaitingApproval,
    /// Popup list is open for selection
    SelectingModel,
    /// Confirm exit dialog
    ConfirmExit,
    /// Edit message selection mode with fuzzy filtering
    EditMessageSelection,
}

/// Main TUI application state
pub struct Tui {
    /// Terminal instance
    terminal: Terminal<CrosstermBackend<Stdout>>,
    terminal_size: (u16, u16),
    /// Current input mode
    input_mode: InputMode,
    /// Text area for message input
    textarea: TextArea<'static>,
    /// Previous insert mode text (for Esc handling)
    previous_insert_text: String,
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
    /// Last time spinner was updated
    last_spinner_update: Instant,
    /// Current tool approval request
    current_tool_approval: Option<ToolCall>,
    /// Available models for selection
    models: Vec<Model>,
    /// Current model in use
    current_model: Model,
    /// Popup selection state (when showing model list)
    popup_state: PopupState,
    /// Cached renderer for performance
    cached_renderer: CachedContentRenderer,
    /// Current UI theme/mode
    /// Event processing pipeline
    event_pipeline: EventPipeline,
    /// Message view model (data + ui state)
    view_model: MessageViewModel,
    /// Edit message selection state
    edit_selection_index: usize,
    edit_selection_messages: Vec<(String, String)>, // (id, content)
}

impl Tui {
    /// Create a new TUI instance
    pub async fn new(
        command_sink: Arc<dyn AppCommandSink>,
        _event_source: Arc<dyn AppEventSource>,
        current_model: Model,
        models: Vec<Model>,
    ) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
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

        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Type your message here...");
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

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
            textarea,
            previous_insert_text: String::new(),
            editing_message_id: None,
            command_sink,
            is_processing: false,
            progress_message: None,
            spinner_state: 0,
            last_spinner_update: Instant::now(),
            current_tool_approval: None,
            models,
            current_model,
            popup_state: PopupState::default(),
            cached_renderer: CachedContentRenderer::new(Arc::new(RwLock::new(ContentCache::new()))),

            event_pipeline: Self::create_event_pipeline(),
            view_model: MessageViewModel::new(),
            edit_selection_index: 0,
            edit_selection_messages: Vec::new(),
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

        // No need to request current conversation - messages are restored in the constructor

        let (term_event_tx, mut term_event_rx) = mpsc::channel::<Result<Event>>(1);
        let _input_handle: JoinHandle<()> = tokio::spawn(async move {
            loop {
                // Poll for a short duration to avoid blocking the task indefinitely
                if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                    let read_result = event::read().map_err(anyhow::Error::from);
                    if term_event_tx.send(read_result).await.is_err() {
                        // Receiver dropped, exit task
                        break;
                    }
                }
            }
        });

        let mut should_exit = false;
        let mut needs_redraw = true; // Force initial draw
        let mut last_spinner_char = String::new();

        while !should_exit {
            // Check if spinner needs update
            let spinner_updated = if self.is_processing
                && self.last_spinner_update.elapsed() > SPINNER_UPDATE_INTERVAL
            {
                self.spinner_state = self.spinner_state.wrapping_add(1);
                self.last_spinner_update = Instant::now();

                // Check if spinner character changed
                let current_spinner = get_spinner_char(self.spinner_state);
                let changed = current_spinner != last_spinner_char;
                last_spinner_char = current_spinner.to_string();
                changed
            } else {
                false
            };

            // Determine if we need to redraw
            if needs_redraw || spinner_updated {
                self.draw()?;
                needs_redraw = false;
            }

            // Prioritize terminal events over app events for responsiveness
            let timeout = if self.is_processing {
                Duration::from_millis(50) // Faster polling when processing for spinner
            } else {
                Duration::from_millis(100)
            };

            tokio::select! {
                Some(event) = term_event_rx.recv() => {
                    match event? {
                        Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                            if self.handle_key_event(key_event).await? {
                                should_exit = true;
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
                            self.view_model.clear_content_cache();
                            needs_redraw = true;
                        }
                        Event::Paste(data) => {
                            // Handle paste in modes that accept text input
                            if matches!(self.input_mode, InputMode::Insert | InputMode::BashCommand) {
                                let normalized_data = data.replace("\r\n", "\n").replace('\r', "\n");
                                self.textarea.insert_str(&normalized_data);
                                debug!(target:"tui.run", "Pasted {} chars in {:?} mode", normalized_data.len(), self.input_mode);
                                needs_redraw = true;
                            }
                        }
                        _ => {}
                    }
                }
                Some(app_event) = event_rx.recv() => {
                    self.handle_app_event(app_event).await;
                    needs_redraw = true;
                }
                _ = tokio::time::sleep(timeout) => {
                    // Timeout for spinner updates when processing
                }
            }
        }

        // Cleanup terminal on exit
        self.cleanup_terminal()?;
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
            let input_mode = self.input_mode;
            let is_processing = self.is_processing;
            let spinner_state = self.spinner_state;
            let current_tool_approval = self.current_tool_approval.as_ref();
            let current_model_owned = self.current_model;
            let textarea_ref = &self.textarea;

            // Clone the required fields to avoid borrowing conflicts
            let (models_clone, _popup_state_clone) =
                (self.models.clone(), self.popup_state.clone());

            // Get chat items from the chat store
            let chat_items = self.view_model.chat_store.as_slice();

            if let Err(e) = Tui::render_ui_static(
                f,
                textarea_ref,
                chat_items,
                &mut self.view_model.chat_list_state,
                input_mode,
                &current_model_owned,
                current_tool_approval,
                &self.cached_renderer,
                self.view_model.current_thread,
                is_processing,
                spinner_state,
                &self.edit_selection_messages,
                self.edit_selection_index,
            ) {
                error!(target:"tui.run.draw", "UI rendering failed: {}", e);
            }

            // Render popup after main UI
            if input_mode == InputMode::SelectingModel {
                Self::render_popup_static(
                    f,
                    f.area(),
                    &models_clone,
                    &current_model_owned,
                    &mut self.popup_state.clone(),
                );
            }

            // Progress is now shown in the status bar, no overlay needed
        })?;
        Ok(())
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
        if let Err(e) = self.event_pipeline.process_event(event, &mut ctx) {
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
            self.view_model.clear_content_cache();
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

    async fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool> {
        match self.input_mode {
            InputMode::Normal => self.handle_normal_mode(key).await,
            InputMode::Insert => self.handle_insert_mode(key).await,
            InputMode::BashCommand => self.handle_bash_mode(key).await,
            InputMode::AwaitingApproval => self.handle_approval_mode(key).await,
            InputMode::SelectingModel => self.handle_model_selection_mode(key).await,
            InputMode::ConfirmExit => self.handle_confirm_exit_mode(key).await,
            InputMode::EditMessageSelection => self.handle_edit_selection_mode(key).await,
        }
    }

    async fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('i') => {
                self.input_mode = InputMode::Insert;
                self.previous_insert_text = self.textarea.lines().join("\n");
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.view_model.chat_list_state.scroll_down(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.view_model.chat_list_state.scroll_up(1);
            }
            KeyCode::Char('g') => {
                self.view_model.chat_list_state.select_first();
            }
            KeyCode::Char('G') => {
                self.view_model
                    .chat_list_state
                    .select_last(self.view_model.chat_store.len());
            }
            KeyCode::Char('e') => {
                // Enter edit message selection mode
                self.enter_edit_selection_mode();
            }
            KeyCode::PageUp => {
                self.view_model.chat_list_state.scroll_up(10);
            }
            KeyCode::PageDown => {
                self.view_model.chat_list_state.scroll_down(10);
            }
            KeyCode::Char('d') => {
                let page_size = self.terminal_size.1.saturating_sub(6) / 2;
                self.view_model.chat_list_state.scroll_down(page_size);
            }
            KeyCode::Char('u') => {
                let page_size = self.terminal_size.1.saturating_sub(6) / 2;
                self.view_model.chat_list_state.scroll_up(page_size);
            }
            KeyCode::Home => {
                self.view_model.chat_list_state.scroll_to_top();
            }
            KeyCode::End => {
                self.view_model.chat_list_state.scroll_to_bottom();
            }
            KeyCode::Char('v') => {
                self.view_model.chat_list_state.view_mode =
                    match self.view_model.chat_list_state.view_mode {
                        ViewMode::Compact => ViewMode::Detailed,
                        ViewMode::Detailed => ViewMode::Compact,
                    };
                self.view_model.clear_content_cache();
            }
            KeyCode::Char('D') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.view_model.chat_list_state.view_mode = ViewMode::Detailed;
                self.view_model.clear_content_cache();
            }
            KeyCode::Char('C') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.view_model.chat_list_state.view_mode = ViewMode::Compact;
                self.view_model.clear_content_cache();
            }
            KeyCode::Esc => {
                // Cancel current processing if any
                if self.is_processing {
                    self.command_sink
                        .send_command(AppCommand::CancelProcessing)
                        .await?;
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.is_processing {
                    // Cancel processing
                    self.command_sink
                        .send_command(AppCommand::CancelProcessing)
                        .await?;
                } else {
                    // Enter exit confirmation mode
                    self.input_mode = InputMode::ConfirmExit;
                }
            }
            KeyCode::Char('!') => {
                // Enter bash command mode
                self.input_mode = InputMode::BashCommand;
            }
            KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl-M: Show model selection popup
                self.input_mode = InputMode::SelectingModel;
                self.popup_state = PopupState::default();
                // Find current model index
                if let Some(index) = self.models.iter().position(|m| m == &self.current_model) {
                    self.popup_state.select(Some(index));
                }
            }
            _ => {}
        }
        Ok(false)
    }

    async fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<bool> {
        let input = Input::from(key);

        // Check for Ctrl+C
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.is_processing {
                // Cancel processing
                self.command_sink
                    .send_command(AppCommand::CancelProcessing)
                    .await?;
            } else {
                // Go to exit confirmation mode
                self.input_mode = InputMode::ConfirmExit;
            }
            return Ok(false);
        }

        // Check for Alt+Enter before passing to textarea
        if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::ALT {
            // Send message
            let content = self.textarea.lines().join("\n");
            if !content.trim().is_empty() {
                self.send_message(content).await?;
                self.textarea = TextArea::default(); // Clear after sending
                self.input_mode = InputMode::Normal;
            }
            return Ok(false);
        }

        match input {
            Input { key: Key::Esc, .. } => {
                // Return to normal mode without clearing text
                self.input_mode = InputMode::Normal;
            }
            _ => {
                // Let textarea handle the input
                self.textarea.input(input);
            }
        }
        Ok(false)
    }

    async fn handle_approval_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some(tool_call) = self.current_tool_approval.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Approve once
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approved: true,
                            always: false,
                        })
                        .await?;
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    // Approve always
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approved: true,
                            always: true,
                        })
                        .await?;
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Reject
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approved: false,
                            always: false,
                        })
                        .await?;
                    self.input_mode = InputMode::Normal;
                }
                _ => {
                    // Put it back if not handled
                    self.current_tool_approval = Some(tool_call);
                }
            }
        } else {
            // No approval pending, return to normal
            self.input_mode = InputMode::Normal;
        }
        Ok(false)
    }

    async fn handle_bash_mode(&mut self, key: KeyEvent) -> Result<bool> {
        // Check for Ctrl+C
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.is_processing {
                // Cancel processing
                self.command_sink
                    .send_command(AppCommand::CancelProcessing)
                    .await?;
            } else {
                // Cancel bash mode and return to normal without clearing text
                self.input_mode = InputMode::Normal;
            }
            return Ok(false);
        }

        let input = Input::from(key);

        match input {
            Input { key: Key::Esc, .. } => {
                // Return to normal mode without clearing text
                self.input_mode = InputMode::Normal;
            }
            Input {
                key: Key::Enter, ..
            } => {
                // Execute the bash command
                let command = self.textarea.lines().join("\n");
                if !command.trim().is_empty() {
                    self.command_sink
                        .send_command(AppCommand::ExecuteBashCommand { command })
                        .await?;
                    self.textarea = TextArea::default(); // Clear after executing
                    self.input_mode = InputMode::Normal;
                }
            }
            _ => {
                // Let textarea handle the input
                self.textarea.input(input);
            }
        }
        Ok(false)
    }

    async fn handle_model_selection_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                if let Some(selected) = self.popup_state.selected() {
                    if selected < self.models.len() {
                        let new_model = self.models[selected];
                        self.current_model = new_model;
                        // Send model change as a slash command
                        let command = format!("/model {}", new_model);
                        self.command_sink
                            .send_command(AppCommand::ExecuteCommand(command))
                            .await?;
                    }
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_state.previous();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.popup_state.next(self.models.len());
            }
            _ => {}
        }
        Ok(false)
    }

    async fn handle_confirm_exit_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // User confirmed exit
                return Ok(true);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+C again also confirms exit
                return Ok(true);
            }
            _ => {
                // Any other key cancels exit and returns to normal mode
                self.input_mode = InputMode::Normal;
            }
        }
        Ok(false)
    }

    async fn send_message(&mut self, content: String) -> Result<()> {
        // Handle slash commands
        if content.starts_with('/') {
            return self.handle_slash_command(content).await;
        }

        // Check if we're editing a message
        if let Some(message_id_to_edit) = self.editing_message_id.take() {
            // Send edit command which creates a new branch
            self.command_sink
                .send_command(AppCommand::EditMessage {
                    message_id: message_id_to_edit,
                    new_content: content,
                })
                .await?;
        } else {
            // Send regular message
            self.command_sink
                .send_command(AppCommand::ProcessUserInput(content))
                .await?;
        }
        Ok(())
    }

    async fn handle_slash_command(&mut self, command: String) -> Result<()> {
        // Just send the command as-is to ExecuteCommand
        self.command_sink
            .send_command(AppCommand::ExecuteCommand(command))
            .await?;
        Ok(())
    }

    /// Enter edit mode for a specific message
    fn enter_edit_mode(&mut self, message_id: &str) {
        // Find the message in the store
        if let Some(item) = self
            .view_model
            .chat_store
            .get_by_id(&message_id.to_string())
        {
            if let crate::tui::model::ChatItem::Message(row) = item {
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
                    self.textarea = TextArea::from(text.lines().collect::<Vec<_>>());
                    self.input_mode = InputMode::Insert;
                    self.previous_insert_text = text;

                    // Store the message ID we're editing
                    self.editing_message_id = Some(message_id.to_string());
                }
            }
        }
    }

    /// Enter edit message selection mode
    fn enter_edit_selection_mode(&mut self) {
        self.input_mode = InputMode::EditMessageSelection;
        self.edit_selection_index = 0;

        // Collect all user messages
        let user_messages: Vec<(String, String)> = self
            .view_model
            .chat_store
            .as_slice()
            .iter()
            .filter_map(|item| {
                if let crate::tui::model::ChatItem::Message(row) = item {
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
                        Some((row.inner.id().to_string(), text))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        self.edit_selection_messages = user_messages;
    }

    /// Handle edit message selection mode input
    async fn handle_edit_selection_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                // Exit edit selection mode
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                // Select the currently highlighted message
                if !self.edit_selection_messages.is_empty()
                    && self.edit_selection_index < self.edit_selection_messages.len()
                {
                    let (message_id, _) =
                        self.edit_selection_messages[self.edit_selection_index].clone();
                    self.enter_edit_mode(&message_id);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                // Move selection up
                if self.edit_selection_index > 0 {
                    self.edit_selection_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Move selection down
                if self.edit_selection_index + 1 < self.edit_selection_messages.len() {
                    self.edit_selection_index += 1;
                }
            }
            _ => {}
        }
        Ok(false)
    }

    /// Static UI rendering function
    #[allow(clippy::too_many_arguments)]
    fn render_ui_static(
        f: &mut Frame,
        textarea: &TextArea,
        chat_items: &[crate::tui::model::ChatItem],
        chat_list_state: &mut ChatListState,
        input_mode: InputMode,
        current_model: &Model,
        current_approval: Option<&ToolCall>,
        _cached_renderer: &CachedContentRenderer,
        _current_thread: Option<uuid::Uuid>,
        is_processing: bool,
        spinner_state: usize,
        edit_selection_messages: &[(String, String)],
        edit_selection_index: usize,
    ) -> Result<()> {
        let size = f.area();

        // Main layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // Messages area (flexible)
                Constraint::Length(5), // Input area
                Constraint::Length(1), // Status bar
            ])
            .split(size);

        // Render messages
        let _message_block = Block::default()
            .borders(Borders::ALL)
            .title(" Messages ")
            .style(Style::default().fg(ratatui::style::Color::DarkGray));

        // Use the ChatList widget as a stateful widget
        let chat_list = ChatList::new(chat_items);
        f.render_stateful_widget(chat_list, chunks[0], chat_list_state);

        // Render input area or approval prompt
        if let Some(tool_call) = current_approval {
            // Use the formatter to create a nice preview
            let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
            let preview_lines = formatter.compact(
                &tool_call.parameters,
                &None,
                (chunks[1].width.saturating_sub(4)) as usize,
            );

            let mut approval_text = vec![
                Line::from(vec![
                    Span::styled("Tool '", Style::default().fg(Color::White)),
                    Span::styled(
                        &tool_call.name,
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("' requests approval:", Style::default().fg(Color::White)),
                ]),
                Line::from(""),
            ];

            // Add the formatted preview
            approval_text.extend(preview_lines);

            // Create the title with embedded options
            let title = Line::from(vec![
                Span::raw(" Tool Approval Required "),
                Span::raw("─ "),
                Span::styled("[Y]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" once "),
                Span::styled("[A]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw("lways "),
                Span::styled("[N]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw("o "),
            ]);

            let approval_block = Paragraph::new(approval_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .style(Style::default().fg(ratatui::style::Color::Yellow)),
                )
                .style(Style::default().fg(ratatui::style::Color::White));
            f.render_widget(approval_block, chunks[1]);
        } else {
            // Render input with a proper block
            let input_block = Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    "{}{}",
                    if is_processing {
                        format!(" {}", get_spinner_char(spinner_state))
                    } else {
                        String::new()
                    },
                    match input_mode {
                        InputMode::Insert => " Insert (Alt-Enter to send, Esc to cancel) ",
                        InputMode::Normal => " (i to insert, ! for bash, u/d/j/k to scroll) ",
                        InputMode::BashCommand => " Bash (Enter to execute, Esc to cancel) ",
                        InputMode::AwaitingApproval => " Awaiting Approval",
                        InputMode::SelectingModel => " Model Selection",
                        InputMode::ConfirmExit =>
                            " Really quit? (y/Y to confirm, any other key to cancel) ",
                        InputMode::EditMessageSelection =>
                            " Select Message To Edit (↑↓ to navigate, Enter to select, Esc to cancel) ",
                    }
                ))
                .style(match input_mode {
                    InputMode::Insert => Style::default().fg(ratatui::style::Color::Green),
                    InputMode::Normal => Style::default().fg(ratatui::style::Color::DarkGray),
                    InputMode::BashCommand => Style::default().fg(ratatui::style::Color::Cyan),
                    InputMode::ConfirmExit => Style::default().fg(ratatui::style::Color::Red),
                    InputMode::EditMessageSelection => {
                        Style::default().fg(ratatui::style::Color::Yellow)
                    }
                    _ => Style::default(),
                });

            // Clone the textarea and set the block
            let mut textarea_with_block = textarea.clone();
            textarea_with_block.set_block(input_block.clone());

            // Special rendering for edit message selection mode
            if input_mode == InputMode::EditMessageSelection {
                // Use the entire input area for the message list
                let mut items: Vec<ListItem> = Vec::new();

                if edit_selection_messages.is_empty() {
                    items.push(
                        ListItem::new("No user messages to edit")
                            .style(Style::default().fg(Color::DarkGray)),
                    );
                } else {
                    for (idx, (_, content)) in edit_selection_messages.iter().enumerate() {
                        let preview = content
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(chunks[1].width.saturating_sub(4) as usize)
                            .collect::<String>();

                        let style = if idx == edit_selection_index {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                                .add_modifier(Modifier::REVERSED)
                        } else {
                            Style::default()
                        };

                        items.push(ListItem::new(preview).style(style));
                    }
                }

                let list = List::new(items).block(input_block);
                f.render_widget(list, chunks[1]);
            } else {
                f.render_widget(&textarea_with_block, chunks[1]);
            }
        }

        let status_block = Paragraph::new(format!(" {} ", current_model))
            .style(Style::default().fg(ratatui::style::Color::Gray))
            .alignment(ratatui::layout::Alignment::Right);
        f.render_widget(status_block, chunks[2]);

        Ok(())
    }

    /// Render model selection popup
    fn render_popup_static(
        f: &mut Frame,
        area: Rect,
        models: &[Model],
        current_model: &Model,
        _popup_state: &mut PopupState,
    ) {
        let items: Vec<String> = models
            .iter()
            .map(|m| {
                if m == current_model {
                    format!("● {}", m)
                } else {
                    format!("  {}", m)
                }
            })
            .collect();

        let popup = PopupList::new("Select Model (Esc to cancel)", &items);

        f.render_widget(popup, centered_rect(50, 70, area));
    }
}

/// Helper function to get spinner character
fn get_spinner_char(state: usize) -> &'static str {
    const SPINNER_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    SPINNER_CHARS[state % SPINNER_CHARS.len()]
}

/// Helper function to create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use conductor_core::app::AppCommand;
    use conductor_core::app::conversation::ToolResult;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc;

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

    fn create_test_tui() -> Tui {
        let command_sink = Arc::new(MockCommandSink);
        let event_source = Arc::new(MockEventSource);

        Tui {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout())).unwrap(),
            terminal_size: (80, 24),
            input_mode: InputMode::Normal,
            textarea: TextArea::default(),
            previous_insert_text: String::new(),
            editing_message_id: None,
            command_sink,
            is_processing: false,
            progress_message: None,
            spinner_state: 0,
            last_spinner_update: Instant::now(),
            current_tool_approval: None,
            models: vec![Model::Claude3_5Sonnet20241022],
            current_model: Model::Claude3_5Sonnet20241022,
            popup_state: PopupState::default(),
            cached_renderer: CachedContentRenderer::new(Arc::new(RwLock::new(ContentCache::new()))),
            event_pipeline: Tui::create_event_pipeline(),
            view_model: MessageViewModel::new(),
            edit_selection_index: 0,
            edit_selection_messages: Vec::new(),
        }
    }

    #[test]
    fn test_restore_messages_preserves_tool_call_params() {
        let mut tui = create_test_tui();

        // Create messages with a tool call
        let tool_call = conductor_tools::ToolCall {
            id: "test_tool_123".to_string(),
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
            tool_use_id: "test_tool_123".to_string(),
            result: ToolResult::Success {
                output: "File contents...".to_string(),
            },
            timestamp: 1234567891,
            thread_id: assistant_msg.thread_id().clone(),
            parent_message_id: Some("msg_assistant".to_string()),
        };

        let messages = vec![assistant_msg, tool_msg];

        // Restore messages
        tui.restore_messages(messages);

        // Verify tool call was preserved in registry
        let stored_call = tui
            .view_model
            .tool_registry
            .get_tool_call("test_tool_123")
            .expect("Tool call should be in registry");
        assert_eq!(stored_call.name, "view");
        assert_eq!(stored_call.parameters, tool_call.parameters);
    }

    #[test]
    fn test_restore_messages_handles_tool_result_before_assistant() {
        let mut tui = create_test_tui();

        let thread_id = uuid::Uuid::new_v4();

        // Create a Tool message that arrives before its corresponding Assistant message
        let tool_msg = Message::Tool {
            id: "msg_tool".to_string(),
            tool_use_id: "orphan_tool_123".to_string(),
            result: ToolResult::Success {
                output: "Orphaned result".to_string(),
            },
            timestamp: 1234567890,
            thread_id,
            parent_message_id: None,
        };

        // The assistant message with tool call arrives later
        let tool_call = conductor_tools::ToolCall {
            id: "orphan_tool_123".to_string(),
            name: "bash".to_string(),
            parameters: json!({"command": "ls -la"}),
        };

        let assistant_msg = Message::Assistant {
            id: "msg_assistant".to_string(),
            content: vec![AssistantContent::ToolCall {
                tool_call: tool_call.clone(),
            }],
            timestamp: 1234567891,
            thread_id,
            parent_message_id: None,
        };

        let messages = vec![tool_msg, assistant_msg];

        // Restore messages
        tui.restore_messages(messages);

        // Verify the tool message was created with proper tool call info
        assert_eq!(tui.view_model.chat_store.len(), 2);
    }
}

// -------------------------------------------------------------------------------------------------
// Public API for backward compatibility with conductor-cli
// -------------------------------------------------------------------------------------------------

/// Free function for best-effort terminal cleanup (raw mode, alt screen, mouse, etc.)
pub fn cleanup_terminal() {
    use crossterm::{
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
        eprintln!("{}", panic_info);
    }));
}

/// High-level entry point for running the TUI
pub async fn run_tui(
    client: std::sync::Arc<conductor_grpc::GrpcClientAdapter>,
    session_id: Option<String>,
    model: conductor_core::api::Model,
    directory: Option<std::path::PathBuf>,
    system_prompt: Option<String>,
) -> anyhow::Result<()> {
    use conductor_core::app::io::{AppCommandSink, AppEventSource};
    use conductor_core::session::{SessionConfig, SessionToolConfig};
    use std::collections::HashMap;

    // If session_id is provided, resume that session
    if let Some(session_id) = session_id {
        // Activate the existing session
        let (messages, _approved_tools) = client.activate_session(session_id.clone()).await?;
        info!(
            "Activated session: {} with {} messages",
            session_id,
            messages.len()
        );
        println!("Session ID: {}", session_id);

        // Start streaming
        client.start_streaming().await?;

        // Get the event receiver
        let event_rx = client.subscribe().await;

        // Initialize TUI with restored conversation
        let mut tui = Tui::new(
            client.clone() as std::sync::Arc<dyn AppCommandSink>,
            client.clone() as std::sync::Arc<dyn AppEventSource>,
            model,
            vec![model], // For now, just pass the single model
        )
        .await?;

        // Restore messages if we have any
        if !messages.is_empty() {
            tui.restore_messages(messages);
        }

        // Run the TUI
        tui.run(event_rx).await?;
    } else {
        // Create a new session
        let mut session_config = SessionConfig {
            workspace: conductor_core::session::state::WorkspaceConfig::default(),
            tool_config: SessionToolConfig::default(),
            system_prompt,
            metadata: HashMap::new(),
        };

        // Add the initial model to session metadata
        session_config
            .metadata
            .insert("initial_model".to_string(), model.to_string());

        // Set workspace directory if provided
        if let Some(dir) = directory {
            std::env::set_current_dir(dir)?;
        }

        // Create session on server
        let session_id = client.create_session(session_config).await?;
        info!("Created session: {}", session_id);
        println!("Session ID: {}", session_id);

        // Start streaming
        client.start_streaming().await?;

        // Get the event receiver
        let event_rx = client.subscribe().await;

        // Initialize TUI
        let mut tui = Tui::new(
            client.clone() as std::sync::Arc<dyn AppCommandSink>,
            client.clone() as std::sync::Arc<dyn AppEventSource>,
            model,
            vec![model], // For now, just pass the single model
        )
        .await?;

        // Run the TUI
        tui.run(event_rx).await?;
    }

    Ok(())
}
