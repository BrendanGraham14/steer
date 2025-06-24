use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::io::{self, Stdout};
use std::panic;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tui_textarea::TextArea;

use tokio::select;

use crate::api::Model;
use crate::app::AppEvent;
use crate::app::command::AppCommand;
use crate::app::conversation::Message;
use crate::app::io::AppCommandSink;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, error, info, warn};
pub mod events;
pub mod state;
pub mod widgets;

use crate::app::conversation::AssistantContent;
use crate::tui::events::{EventPipeline, processors::*};
use crate::tui::state::content_cache::ContentCache;
use crate::tui::state::view_model::MessageViewModel;
use crate::tui::widgets::{
    CachedContentRenderer, MessageList, MessageListState, ViewMode,
    content_renderer::ContentRenderer, message_list::MessageContent,
};
use widgets::styles;

const MAX_INPUT_HEIGHT: u16 = 10;
const SPINNER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Normal,
    Editing,
    BashCommand,
    AwaitingApproval,
    ConfirmExit,
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    terminal_size: (u16, u16), // Track terminal size for accurate scrolling
    textarea: TextArea<'static>,
    input_mode: InputMode,
    view_model: crate::tui::state::view_model::MessageViewModel,
    is_processing: bool,
    progress_message: Option<String>,
    last_spinner_update: Instant,
    spinner_state: usize,
    command_sink: Arc<dyn AppCommandSink>,
    current_tool_approval: Option<tools::ToolCall>,
    current_model: Model,
    event_pipeline: EventPipeline,
}

#[derive(Debug)]
enum InputAction {
    SendMessage(String),
    ExecuteBashCommand(String),
    ApproveToolNormal(String),
    ApproveToolAlways(String),
    DenyTool(String),
    CancelProcessing,
    Exit,
}

impl Tui {
    pub fn new(command_sink: Arc<dyn AppCommandSink>, initial_model: Model) -> Result<Self> {
        // Detect terminal for diagnostics
        let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        debug!(target:"tui.new", "Terminal detected: {}", term_program);

        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            ),
            EnableBracketedPaste,
            EnableMouseCapture
        )?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        let terminal_size = terminal
            .size()
            .map(|s| (s.width, s.height))
            .unwrap_or((80, 24));
        // Initialize TextArea
        let mut textarea = TextArea::default();
        textarea.set_block(
            ratatui::widgets::Block::default()
                .borders(Borders::ALL)
                .title("Input (Ctrl+S or i to edit, Enter to send, Alt+Enter for newline, Esc to exit)"),
        );
        textarea.set_placeholder_text("Enter your message here...");
        textarea.set_style(Style::default());
        // Set cursor style to make it more visible
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

        Ok(Self {
            terminal,
            terminal_size,
            textarea,
            input_mode: InputMode::Normal,
            view_model: MessageViewModel::new(),
            is_processing: false,
            progress_message: None,
            last_spinner_update: Instant::now(),
            spinner_state: 0,
            command_sink,
            current_tool_approval: None,
            current_model: initial_model,
            event_pipeline: Self::create_event_pipeline(),
        })
    }

    /// Create a TUI with restored conversation state for local sessions
    pub fn new_with_conversation(
        command_sink: Arc<dyn AppCommandSink>,
        initial_model: Model,
        messages: Vec<Message>,
        _approved_tools: Vec<String>, // TODO: restore approved tools
    ) -> Result<Self> {
        let mut tui = Self::new(command_sink, initial_model)?;
        tui.restore_messages(messages);
        Ok(tui)
    }

    /// Restore messages to the TUI, properly populating the tool registry
    fn restore_messages(&mut self, messages: Vec<Message>) {
        let message_count = messages.len();
        info!("Starting to restore {} messages to TUI", message_count);

        // Debug: log all Tool messages to check their IDs
        for message in &messages {
            if let crate::app::Message::Tool { tool_use_id, .. } = message {
                debug!(
                    target: "tui.restore",
                    "Found Tool message with tool_use_id={}",
                    tool_use_id
                );
            }
        }

        // First pass: populate tool registry with tool calls from assistant messages
        for message in &messages {
            if let crate::app::Message::Assistant { content, .. } = message {
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

        for (i, message) in messages.into_iter().enumerate() {
            let content = Self::convert_message(message, &self.view_model.tool_registry);

            if i < 5 || i >= message_count - 5 {
                info!(
                    "Converting message {}: type={:?}",
                    i,
                    match &content {
                        MessageContent::User { .. } => "User",
                        MessageContent::Assistant { .. } => "Assistant",
                        MessageContent::Tool { .. } => "Tool",
                    }
                );
            }

            match &content {
                MessageContent::Tool {
                    id, call, result, ..
                } => {
                    // Merge with existing placeholder or create new
                    let idx = self.get_or_create_tool_index(id, Some(call.name.clone()));
                    if let MessageContent::Tool {
                        call: existing_call,
                        result: existing_result,
                        ..
                    } = &mut self.view_model.messages[idx]
                    {
                        // Always keep latest call params (they should be identical)
                        *existing_call = call.clone();
                        // Attach result if we didn't have it yet
                        if existing_result.is_none() {
                            *existing_result = result.clone();
                        }
                    }
                }
                _ => {
                    self.view_model.add_message(content);
                }
            }
        }

        info!(
            "Finished restoring messages. TUI now has {} messages",
            self.view_model.messages.len()
        );

        // Reset scroll to bottom after restoring messages
        self.view_model.message_list_state.scroll_to_bottom();
    }

    fn cleanup_terminal(&mut self) -> Result<()> {
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
            self.view_model.messages.len()
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
                let new_spinner = self.get_spinner_char();
                let changed = new_spinner != last_spinner_char;
                last_spinner_char = new_spinner;
                changed
            } else {
                false
            };

            // Only redraw if something changed
            if needs_redraw || spinner_updated {
                let input_mode = self.input_mode;
                let is_processing = self.is_processing;
                let progress_message_owned: Option<String> = self.progress_message.clone();
                let spinner_char_owned: String = self.get_spinner_char();
                let current_model_owned: String = self.current_model.to_string();
                let current_tool_approval_info = self.current_tool_approval.clone();

                let input_block = Tui::create_input_block_static(
                    input_mode,
                    is_processing,
                    progress_message_owned,
                    spinner_char_owned,
                    current_tool_approval_info.as_ref(),
                );
                self.textarea.set_block(input_block);

                self.terminal.draw(|f| {
                    let textarea_ref = &self.textarea;
                    let current_approval_ref = self.current_tool_approval.as_ref();

                    // Get references separately to avoid borrow conflicts
                    let messages_slice = self.view_model.messages.as_slice();
                    let content_cache = self.view_model.content_cache();

                    if let Err(e) = Tui::render_ui_static(
                        f,
                        textarea_ref,
                        messages_slice,
                        &mut self.view_model.message_list_state,
                        input_mode,
                        &current_model_owned,
                        current_approval_ref,
                        content_cache,
                    ) {
                        error!(target:"tui.run.draw", "UI rendering failed: {}", e);
                    }
                })?;

                needs_redraw = false; // Reset after drawing
            }

            select! {
                // Prioritize terminal events (especially mouse events) for responsiveness
                biased;

                maybe_term_event = term_event_rx.recv() => {
                    match maybe_term_event {
                        Some(Ok(event)) => {
                            match event {
                                Event::Resize(width, height) => {
                                    debug!(target:"tui.run", "Terminal resized to {}x{}", width, height);
                                    self.terminal_size = (width, height);

                                    needs_redraw = true;
                                    // The widget will handle recalculating sizes internally
                                }
                                Event::Paste(data) => {
                                    if matches!(self.input_mode, InputMode::Editing) {
                                        let normalized_data = data.replace("\r\n", "\n").replace("\r", "\n");
                                        self.textarea.insert_str(&normalized_data);
                                        debug!(target:"tui.run", "Pasted {} chars", normalized_data.len());
                                        needs_redraw = true;
                                    }
                                }
                                Event::Key(key) => {
                                    match self.handle_input(key).await {
                                        Ok(Some(action)) => {
                                            debug!(target:"tui.run", "Handling input action: {:?}", action);
                                            if self.dispatch_input_action(action).await? {
                                                should_exit = true;
                                            }
                                            needs_redraw = true;
                                        }
                                        Ok(None) => {
                                            // Some keys (like navigation) are handled in handle_input
                                            // Check if they changed state
                                            needs_redraw = true; // Conservative: assume state changed
                                        }
                                        Err(e) => {
                                            error!(target:"tui.run",    "Error handling input: {}", e);
                                        }
                                    }
                                }
                                Event::FocusGained => debug!(target:"tui.run", "Focus gained"),
                                Event::FocusLost => debug!(target:"tui.run", "Focus lost"),
                                Event::Mouse(event) => {
                                    // Fast path for mouse events to minimize latency
                                    // Mouse scroll events are handled with early return to prevent
                                    // processing of other events in the same iteration. This ensures
                                    // clean scroll behavior without interference from other inputs.
                                    match event {
                                        MouseEvent {
                                            kind: MouseEventKind::ScrollDown,
                                            ..
                                        } => {
                                            let scrolled = self.view_model.message_list_state.simple_scroll_down(
                                                3, // Scroll by 3 lines for mouse wheel
                                                self.view_model.messages.as_slice(),
                                                Some(self.terminal_size),
                                            );
                                            if scrolled {
                                                needs_redraw = true;
                                            }
                                            continue;
                                        }
                                        MouseEvent {
                                            kind: MouseEventKind::ScrollUp,
                                            ..
                                        } => {
                                            let scrolled = self.view_model.message_list_state.simple_scroll_up(
                                                3, // Scroll by 3 lines for mouse wheel
                                                self.view_model.messages.as_slice(),
                                                Some(self.terminal_size),
                                            );
                                            if scrolled {
                                                needs_redraw = true;
                                            }
                                            continue;
                                        }
                                        _ => {} // Ignore other mouse events
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(target:"tui.run", "Error reading terminal event: {}", e);
                            // Decide if we should exit on error
                            // should_exit = true;
                        }
                        None => {
                            // Channel closed, input task likely ended
                            info!(target:"tui.run", "Terminal event channel closed.");
                            should_exit = true;
                        }
                    }
                }

                maybe_app_event = event_rx.recv() => {
                    match maybe_app_event {
                        Some(event) => {
                            self.handle_app_event(event).await;
                            needs_redraw = true; // App events usually require redraw
                        }
                        None => {
                            info!(target:"tui.run", "App event channel closed.");
                            should_exit = true;
                        }
                    }
                }

                _ = tokio::time::sleep(SPINNER_UPDATE_INTERVAL / 2), if self.is_processing => {
                    // Timer fired, spinner will update on next iteration
                    needs_redraw = true;
                }
            }
        }

        // Cleanup terminal before exiting run loop
        self.cleanup_terminal()?;
        Ok(())
    }

    /// Create the event processing pipeline with all processors
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
            messages: &mut self.view_model.messages,
            message_list_state: &mut self.view_model.message_list_state,
            tool_registry: &mut self.view_model.tool_registry,
            command_sink: &self.command_sink,
            is_processing: &mut self.is_processing,
            progress_message: &mut self.progress_message,
            spinner_state: &mut self.spinner_state,
            current_tool_approval: &mut self.current_tool_approval,
            current_model: &mut self.current_model,
            messages_updated: &mut messages_updated,
        };

        // Process the event through the pipeline
        if let Err(e) = self.event_pipeline.process_event(event, &mut ctx) {
            tracing::error!(target: "tui.handle_app_event", "Event processing failed: {}", e);
        }

        // Handle special input mode changes for tool approval
        if self.current_tool_approval.is_some() && self.input_mode != InputMode::AwaitingApproval {
            self.input_mode = InputMode::AwaitingApproval;
        } else if self.current_tool_approval.is_none()
            && self.input_mode == InputMode::AwaitingApproval
        {
            self.input_mode = InputMode::Normal;
        }

        // Auto-scroll to bottom if messages were updated and user hasn't manually scrolled
        if messages_updated && !self.view_model.message_list_state.user_scrolled {
            // Auto-scroll to bottom
            self.view_model.message_list_state.scroll_to_bottom();
        }
    }

    fn convert_message(
        message: crate::app::Message,
        tool_registry: &crate::tui::state::tool_registry::ToolCallRegistry,
    ) -> MessageContent {
        match message {
            crate::app::Message::User {
                content,
                timestamp,
                id,
            } => MessageContent::User {
                id,
                blocks: content,
                timestamp: chrono::DateTime::from_timestamp(timestamp as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| timestamp.to_string()),
            },
            crate::app::Message::Assistant {
                content,
                timestamp,
                id,
            } => {
                // Don't convert single tool calls to Tool messages during restore
                // The actual Tool message will provide the complete information
                MessageContent::Assistant {
                    id,
                    blocks: content,
                    timestamp: chrono::DateTime::from_timestamp(timestamp as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| timestamp.to_string()),
                }
            }
            crate::app::Message::Tool {
                tool_use_id,
                result,
                timestamp,
                id: _,
            } => {
                debug!(
                    target: "tui.convert",
                    "Converting Tool message: tool_use_id={}",
                    tool_use_id
                );

                // Try to find the corresponding ToolCall that we cached earlier
                let tool_call = tool_registry
                    .get_tool_call(&tool_use_id)
                    .cloned()
                    .inspect(|tc| {
                        debug!(
                            target: "tui.convert",
                            "Found tool call in registry: name={}, params={}",
                            tc.name,
                            tc.parameters
                        );
                    })
                    .unwrap_or_else(|| {
                        warn!(target:"tui.convert_message", "Tool message {} has no associated tool call info", tool_use_id);
                        tools::ToolCall {
                            id: tool_use_id.clone(),
                            name: "unknown".to_string(),
                            parameters: serde_json::Value::Null,
                        }
                    });

                MessageContent::Tool {
                    id: tool_use_id,
                    call: tool_call,
                    result: Some(result),
                    timestamp: chrono::DateTime::from_timestamp(timestamp as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| timestamp.to_string()),
                }
            }
        }
    }

    fn get_spinner_char(&self) -> String {
        const SPINNER: [&str; 4] = ["⠋", "⠙", "⠹", "⠸"];
        SPINNER[self.spinner_state % SPINNER.len()].to_string()
    }

    /// Ensure there is exactly one MessageContent::Tool for the given id.
    /// Returns the index into self.view_model.messages for that message. If it does not
    /// exist yet, a placeholder is created (optionally with a provided name).
    fn get_or_create_tool_index(&mut self, id: &str, name_hint: Option<String>) -> usize {
        debug!(
            target: "tui.tool_index",
            "get_or_create_tool_index called for id={}, name_hint={:?}",
            id, name_hint
        );

        if let Some(idx) = self.view_model.tool_registry.get_message_index(id) {
            debug!(target: "tui.tool_index", "Found existing message at index {}", idx);
            return idx;
        }

        // Try to reuse any existing ToolCall info from registry (may already have parameters)
        let existing = self.view_model.tool_registry.get_tool_call(id).cloned();

        if let Some(ref call) = existing {
            debug!(
                target: "tui.tool_index",
                "Found existing tool call in registry: name={}, params={}",
                call.name, call.parameters
            );
        } else {
            debug!(
                target: "tui.tool_index",
                "No existing tool call found in registry for id={}",
                id
            );
        }

        // Build placeholder ToolCall, preferring existing parameters if available
        let placeholder_call = existing.unwrap_or_else(|| tools::ToolCall {
            id: id.to_string(),
            name: name_hint.unwrap_or_else(|| "unknown".to_string()),
            parameters: serde_json::Value::Null,
        });

        debug!(
            target: "tui.tool_index",
            "Creating Tool message with call: name={}, params={}",
            placeholder_call.name, placeholder_call.parameters
        );

        self.view_model.messages.push(MessageContent::Tool {
            id: id.to_string(),
            call: placeholder_call.clone(),
            result: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

        let idx = self.view_model.messages.len() - 1;
        self.view_model.tool_registry.set_message_index(id, idx);
        self.view_model
            .tool_registry
            .register_call(placeholder_call);

        idx
    }

    fn create_input_block_static<'a>(
        input_mode: InputMode,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_char: String,
        current_tool_approval_info: Option<&tools::ToolCall>,
    ) -> Block<'a> {
        let input_border_style = match input_mode {
            InputMode::Editing => styles::INPUT_BORDER_ACTIVE,
            InputMode::Normal => styles::INPUT_BORDER_NORMAL,
            InputMode::BashCommand => styles::INPUT_BORDER_COMMAND,
            InputMode::AwaitingApproval => {
                // Only style as ApprovalRequired if there *is* a current approval
                if current_tool_approval_info.is_some() {
                    styles::INPUT_BORDER_APPROVAL
                } else {
                    styles::INPUT_BORDER_NORMAL
                }
            }
            InputMode::ConfirmExit => styles::INPUT_BORDER_EXIT,
        };
        let input_title: String = match input_mode {
            InputMode::Editing => "Input (Esc to stop editing, Enter to send, Alt+Enter for newline)".to_string(),
            InputMode::Normal => {
                "Normal (i=edit, j/k=scroll, d/u=½page, G/End=bottom, v=view mode, Shift+D/C=detail/compact, Ctrl+C=exit)".to_string()
            }
            InputMode::BashCommand => "Bash Command (Enter to execute, Esc to cancel)".to_string(),
            InputMode::AwaitingApproval => {
                // Update title based on current approval info
                if let Some(info) = current_tool_approval_info {
                    format!(
                        "Approve Tool: '{}'? (y/n, Shift+Tab=always, Esc=deny)",
                        info.name
                    )
                    .to_string()
                } else {
                    // Fallback title if somehow in this mode without current approval
                    "Awaiting Approval... (State Error?)".to_string()
                }
            }
            InputMode::ConfirmExit => "Really quit? (y/N)".to_string(),
        };

        let title_line = if is_processing {
            let progress_msg = progress_message.as_deref().unwrap_or("Processing...");

            let processing_span = Span::styled(
                format!("{} {} ", &spinner_char, progress_msg),
                Style::default().white(),
            );

            let input_title_span = Span::styled(input_title, Style::default());

            Line::from(vec![processing_span, input_title_span])
        } else {
            let title_span = Span::raw(input_title);
            Line::from(vec![title_span])
        };

        Block::<'a>::default()
            .borders(Borders::ALL)
            .title(title_line)
            .style(input_border_style)
    }

    fn render_ui_static(
        f: &mut ratatui::Frame<'_>,
        textarea: &TextArea<'_>,
        messages: &[MessageContent],
        view_state: &mut MessageListState,
        input_mode: InputMode,
        current_model: &str,
        current_approval_info: Option<&tools::ToolCall>,
        content_cache: std::sync::Arc<std::sync::RwLock<ContentCache>>,
    ) -> Result<()> {
        let total_area = f.area();
        let input_height = (textarea.lines().len() as u16 + 2)
            .min(MAX_INPUT_HEIGHT)
            .min(total_area.height);

        // Calculate potential preview height
        let preview_height = if current_approval_info.is_some() {
            // Calculate dynamic preview height based on terminal size
            // Use up to 30% of terminal height, with min 8 and max 20 lines
            let max_preview = (total_area.height as f32 * 0.3) as u16;
            max_preview.clamp(1, 20)
        } else {
            0
        };

        // Adjust layout constraints
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),                 // Messages area
                Constraint::Length(preview_height), // Tool preview area (optional)
                Constraint::Length(input_height),   // Input area
                Constraint::Length(1),              // Model info area
            ])
            .split(f.area());

        let messages_area = chunks[0];
        let preview_area = chunks[1]; // New area
        let input_area = chunks[2];
        let model_info_area = chunks[3];

        // Pre-calculate visible range to get messages_above count
        let inner_area = messages_area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let cached_renderer = CachedContentRenderer::new(content_cache);
        let visible_range = view_state.calculate_visible_range(
            messages,
            inner_area.height,
            inner_area.width,
            &cached_renderer as &dyn ContentRenderer,
        );

        // Render Messages using the new widget with StatefulWidget pattern
        let message_block = if let Some(range) = &visible_range {
            if range.messages_above > 0 {
                Block::default()
                    .borders(Borders::ALL)
                    .title(Line::from(Span::styled(
                        format!("(↑ {} messages above)", range.messages_above),
                        Style::default().white(),
                    )))
            } else {
                Block::default().borders(Borders::ALL)
            }
        } else {
            Block::default().borders(Borders::ALL)
        };

        let message_widget = MessageList::new(messages)
            .with_renderer(Box::new(cached_renderer))
            .block(message_block);
        f.render_stateful_widget(message_widget, messages_area, view_state);

        // Render Tool Preview (conditionally)
        if let Some(info) = current_approval_info {
            Self::render_tool_preview_static(f, preview_area, info);
        }

        // Render Text Area
        f.render_widget(textarea, input_area);

        // Render model info at the bottom
        let model_info: Paragraph<'_> =
            Paragraph::new(Line::from(Span::styled(current_model, styles::MODEL_INFO)))
                .alignment(ratatui::layout::Alignment::Right);
        f.render_widget(model_info, model_info_area);

        // Hide terminal cursor when in editing mode
        // The textarea widget renders its own cursor, so we don't need the terminal cursor
        if input_mode == InputMode::Editing {
            // Hide cursor by setting it outside the visible area
            f.set_cursor_position(Position { x: 0, y: 0 });
        }

        Ok(())
    }

    // Static function to render the tool preview
    fn render_tool_preview_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        tool_call: &tools::ToolCall,
    ) {
        use crate::tui::widgets::content_renderer::{ContentRenderer, DefaultContentRenderer};

        // Create a temporary Tool message to render
        let temp_message = MessageContent::Tool {
            id: tool_call.id.clone(),
            call: tool_call.clone(),
            result: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        let renderer = DefaultContentRenderer;
        renderer.render(&temp_message, ViewMode::Detailed, area, f.buffer_mut());
    }

    async fn handle_input(&mut self, key: KeyEvent) -> Result<Option<InputAction>> {
        let mut action = None;

        // --- Global keys handled first ---

        // Handle ESC specifically for denying the current tool approval
        if key.code == KeyCode::Esc && self.input_mode == InputMode::AwaitingApproval {
            if let Some(approval_info) = self.current_tool_approval.take() {
                action = Some(InputAction::DenyTool(approval_info.id));
                self.input_mode = InputMode::Normal; // Revert to normal mode
                return Ok(action);
            }
        }

        // --- View mode and navigation keys ---
        if self.input_mode != InputMode::Editing {
            match (key.code, key.modifiers) {
                // View mode toggles
                // v - cycle through view modes
                // Shift+D - force Detailed view
                // Shift+C - force Compact view
                (KeyCode::Char('v'), KeyModifiers::NONE) => {
                    self.view_model
                        .message_list_state
                        .view_prefs
                        .global_override = match self
                        .view_model
                        .message_list_state
                        .view_prefs
                        .global_override
                    {
                        None => Some(ViewMode::Compact),
                        Some(ViewMode::Compact) => Some(ViewMode::Detailed),
                        Some(ViewMode::Detailed) => None,
                    };
                    // Clear caches when view mode changes

                    self.view_model.clear_content_cache();
                    return Ok(None);
                }
                (KeyCode::Char('D'), KeyModifiers::SHIFT)
                    if self.input_mode == InputMode::Normal =>
                {
                    self.view_model
                        .message_list_state
                        .view_prefs
                        .global_override = Some(ViewMode::Detailed);
                    // Clear caches when view mode changes

                    self.view_model.clear_content_cache();
                    return Ok(None);
                }
                (KeyCode::Char('C'), KeyModifiers::SHIFT)
                    if self.input_mode == InputMode::Normal =>
                {
                    self.view_model
                        .message_list_state
                        .view_prefs
                        .global_override = Some(ViewMode::Compact);
                    // Clear cache when view mode changes

                    return Ok(None);
                }

                // Navigation
                (KeyCode::PageUp, _) => {
                    // Scroll up by approximately one page
                    let page_size = self.terminal_size.1.saturating_sub(6) / 2; // Half page
                    let scrolled = self.view_model.message_list_state.simple_scroll_up(
                        page_size,
                        self.view_model.messages.as_slice(),
                        Some(self.terminal_size),
                    );
                    if scrolled {
                        self.view_model.message_list_state.user_scrolled = true;
                    }
                    return Ok(None);
                }
                (KeyCode::PageDown, _) => {
                    // Scroll down by approximately one page
                    let page_size = self.terminal_size.1.saturating_sub(6) / 2; // Half page
                    let scrolled = self.view_model.message_list_state.simple_scroll_down(
                        page_size,
                        self.view_model.messages.as_slice(),
                        Some(self.terminal_size),
                    );
                    if scrolled {
                        self.view_model.message_list_state.user_scrolled = true;
                    }
                    return Ok(None);
                }
                (KeyCode::Home, _) => {
                    // Scroll to top
                    self.view_model.message_list_state.set_scroll_offset(999999); // Will be clamped to max
                    return Ok(None);
                }
                (KeyCode::End, _) => {
                    // Scroll to bottom
                    self.view_model.message_list_state.scroll_to_bottom();
                    return Ok(None);
                }
                (KeyCode::Up, modifiers) => {
                    if modifiers == KeyModifiers::SHIFT {
                        // Select previous message
                        self.view_model.message_list_state.select_previous();
                    } else {
                        // Scroll up
                        let scrolled = self.view_model.message_list_state.simple_scroll_up(
                            1,
                            self.view_model.messages.as_slice(),
                            Some(self.terminal_size),
                        );
                        if scrolled {
                            self.view_model.message_list_state.user_scrolled = true;
                        }
                    }
                    return Ok(None);
                }
                (KeyCode::Down, modifiers) => {
                    if modifiers == KeyModifiers::SHIFT {
                        // Select next message
                        self.view_model
                            .message_list_state
                            .select_next(self.view_model.messages.len());
                    } else {
                        // Scroll down
                        let scrolled = self.view_model.message_list_state.simple_scroll_down(
                            1,
                            self.view_model.messages.as_slice(),
                            Some(self.terminal_size),
                        );
                        if scrolled {
                            self.view_model.message_list_state.user_scrolled = true;
                        }
                    }
                    return Ok(None);
                }

                // Vim-style navigation (only in Normal mode)
                // j/k - scroll down/up one line
                // d/u - scroll down/up half page
                // G - go to bottom, g - go to top
                (KeyCode::Char('g'), KeyModifiers::NONE)
                    if self.input_mode == InputMode::Normal =>
                {
                    self.view_model.message_list_state.set_scroll_offset(999999); // Will be clamped to max
                    return Ok(None);
                }
                (KeyCode::Char('G'), KeyModifiers::SHIFT)
                    if self.input_mode == InputMode::Normal =>
                {
                    self.view_model.message_list_state.scroll_to_bottom();
                    return Ok(None);
                }
                (KeyCode::Char('k'), KeyModifiers::NONE)
                    if self.input_mode == InputMode::Normal =>
                {
                    // Scroll up one line
                    let scrolled = self.view_model.message_list_state.simple_scroll_up(
                        1,
                        self.view_model.messages.as_slice(),
                        Some(self.terminal_size),
                    );
                    if scrolled {
                        self.view_model.message_list_state.user_scrolled = true;
                    }
                    return Ok(None);
                }
                (KeyCode::Char('j'), KeyModifiers::NONE)
                    if self.input_mode == InputMode::Normal =>
                {
                    // Scroll down one line
                    let scrolled = self.view_model.message_list_state.simple_scroll_down(
                        1,
                        self.view_model.messages.as_slice(),
                        Some(self.terminal_size),
                    );
                    if scrolled {
                        self.view_model.message_list_state.user_scrolled = true;
                    }
                    return Ok(None);
                }
                (KeyCode::Char('u'), KeyModifiers::NONE)
                    if self.input_mode == InputMode::Normal =>
                {
                    // Scroll up half page
                    let scrolled = self.view_model.message_list_state.simple_scroll_up(
                        20,
                        self.view_model.messages.as_slice(),
                        Some(self.terminal_size),
                    );
                    if scrolled {
                        self.view_model.message_list_state.user_scrolled = true;
                    }
                    return Ok(None);
                }
                (KeyCode::Char('d'), KeyModifiers::NONE)
                    if self.input_mode == InputMode::Normal =>
                {
                    // Scroll down half page
                    let scrolled = self.view_model.message_list_state.simple_scroll_down(
                        20,
                        self.view_model.messages.as_slice(),
                        Some(self.terminal_size),
                    );
                    if scrolled {
                        self.view_model.message_list_state.user_scrolled = true;
                    }
                    return Ok(None);
                }

                // Toggle expansion of selected message
                (KeyCode::Enter, _) if self.input_mode == InputMode::Normal => {
                    if let Some(idx) = self.view_model.message_list_state.selected {
                        if idx < self.view_model.messages.len() {
                            let msg_id = self.view_model.messages[idx].id().to_string();
                            self.view_model.message_list_state.toggle_expanded(msg_id);
                            return Ok(None);
                        }
                    }
                    // If no selection, send the message
                    let input = self.textarea.lines().join("\n");
                    if !input.trim().is_empty() {
                        self.clear_textarea();
                        action = Some(InputAction::SendMessage(input));
                        return Ok(action);
                    }
                }

                // Toggle message detail (Ctrl+R)
                (KeyCode::Char('r'), KeyModifiers::CONTROL)
                | (KeyCode::Char('R'), KeyModifiers::CONTROL) => {
                    if let Some(idx) = self.view_model.message_list_state.selected {
                        if idx < self.view_model.messages.len() {
                            let msg_id = self.view_model.messages[idx].id().to_string();
                            self.view_model.message_list_state.toggle_expanded(msg_id);
                        }
                    }
                    return Ok(None);
                }

                _ => {}
            }
        }

        // --- Mode-specific keys ---
        match self.input_mode {
            InputMode::Normal => {
                match key.code {
                    KeyCode::Esc => {
                        if self.is_processing {
                            action = Some(InputAction::CancelProcessing);
                        } else {
                            self.clear_textarea();
                        }
                        return Ok(action);
                    }
                    // Enter Edit Mode
                    KeyCode::Char('i') | KeyCode::Char('s') => {
                        self.input_mode = InputMode::Editing;
                    }
                    // Enter Bash Command Mode
                    KeyCode::Char('!') => {
                        self.input_mode = InputMode::BashCommand;
                        self.clear_textarea();
                    }
                    // Cancel Processing (Ctrl+C)
                    KeyCode::Char('c') | KeyCode::Char('C')
                        if key.modifiers == KeyModifiers::CONTROL =>
                    {
                        if self.is_processing {
                            action = Some(InputAction::CancelProcessing);
                        } else {
                            // Enter ConfirmExit mode if not processing
                            self.input_mode = InputMode::ConfirmExit;
                        }
                        return Ok(action);
                    }
                    _ => {} // Other keys handled above
                }
            }
            InputMode::Editing => {
                match key.code {
                    // Stop editing
                    KeyCode::Esc => {
                        self.input_mode = InputMode::Normal;
                    }
                    // Debug: log all Enter key variants
                    KeyCode::Enter => {
                        debug!(target:"tui.input", "Enter key pressed with modifiers: {:?}", key.modifiers);

                        if key.modifiers.contains(KeyModifiers::ALT) {
                            // Insert newline (Alt+Enter)
                            self.textarea.insert_char('\n');
                        } else {
                            // Send Message (plain Enter)
                            let input = self.textarea.lines().join("\n");
                            if !input.trim().is_empty() {
                                action = Some(InputAction::SendMessage(input));
                                // Re-initialize the textarea to clear it
                                let mut new_textarea = TextArea::default();
                                new_textarea
                                    .set_block(self.textarea.block().cloned().unwrap_or_default());
                                new_textarea.set_placeholder_text(self.textarea.placeholder_text());
                                new_textarea.set_style(self.textarea.style());
                                new_textarea.set_cursor_style(
                                    Style::default().add_modifier(Modifier::REVERSED),
                                );
                                self.textarea = new_textarea;
                                self.input_mode = InputMode::Normal;
                            }
                        }
                    }
                    // Cancel Processing (Ctrl+C)
                    KeyCode::Char('c') | KeyCode::Char('C')
                        if key.modifiers == KeyModifiers::CONTROL =>
                    {
                        if self.is_processing {
                            action = Some(InputAction::CancelProcessing);
                        } else {
                            // Enter ConfirmExit mode if not processing
                            self.input_mode = InputMode::ConfirmExit;
                        }
                        return Ok(action);
                    }
                    // Pass other keys to text area
                    _ => {
                        // Pass ratatui KeyEvent directly, as tui-textarea's input() expects `impl Into<Input>`
                        self.textarea.input(key);
                    }
                }
            }
            InputMode::AwaitingApproval => {
                if let Some(approval_info) = self.current_tool_approval.clone() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            action = Some(InputAction::ApproveToolNormal(approval_info.id));
                            self.current_tool_approval = None; // Clear approval
                            self.input_mode = InputMode::Normal; // Revert to normal
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            action = Some(InputAction::DenyTool(approval_info.id));
                            self.current_tool_approval = None; // Clear approval
                            self.input_mode = InputMode::Normal; // Revert to normal
                        }
                        KeyCode::BackTab => {
                            // Shift+Tab for Approve Always
                            action = Some(InputAction::ApproveToolAlways(approval_info.id));
                            self.current_tool_approval = None; // Clear approval
                            self.input_mode = InputMode::Normal; // Revert to normal
                        }
                        // Cancel Processing (Ctrl+C)
                        KeyCode::Char('c') | KeyCode::Char('C')
                            if key.modifiers == KeyModifiers::CONTROL =>
                        {
                            if self.is_processing {
                                action = Some(InputAction::CancelProcessing);
                            } else {
                                // Enter ConfirmExit mode if not processing
                                self.input_mode = InputMode::ConfirmExit;
                            }
                            return Ok(action);
                        }
                        _ => {} // Ignore other keys
                    }
                } else {
                    // Should not happen, but revert to normal mode if no approval is active
                    warn!(target: "tui.handle_input", "In AwaitingApproval mode but no current approval info. Reverting to Normal.");
                    self.input_mode = InputMode::Normal;
                }
            }
            InputMode::BashCommand => {
                match key.code {
                    // Cancel bash command
                    KeyCode::Esc => {
                        self.input_mode = InputMode::Normal;
                        self.clear_textarea();
                    }
                    // Execute bash command
                    KeyCode::Enter => {
                        let command = self.textarea.lines().join("\n");
                        if !command.trim().is_empty() {
                            action = Some(InputAction::ExecuteBashCommand(command));
                            self.clear_textarea();
                            self.input_mode = InputMode::Normal;
                        }
                    }
                    // Cancel Processing (Ctrl+C)
                    KeyCode::Char('c') | KeyCode::Char('C')
                        if key.modifiers == KeyModifiers::CONTROL =>
                    {
                        if self.is_processing {
                            action = Some(InputAction::CancelProcessing);
                        } else {
                            // Cancel bash command and return to normal
                            self.input_mode = InputMode::Normal;
                            self.clear_textarea();
                        }
                        return Ok(action);
                    }
                    // Pass other keys to text area
                    _ => {
                        self.textarea.input(key);
                    }
                }
            }
            InputMode::ConfirmExit => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    action = Some(InputAction::Exit);
                }
                // Also exit if Ctrl+C is pressed *again*
                KeyCode::Char('c') | KeyCode::Char('C')
                    if key.modifiers == KeyModifiers::CONTROL =>
                {
                    action = Some(InputAction::Exit);
                }
                _ => {
                    // Any other key cancels exit confirmation
                    self.input_mode = InputMode::Normal;
                }
            },
        }
        Ok(action)
    }

    async fn dispatch_input_action(&mut self, action: InputAction) -> Result<bool> {
        let mut should_exit = false;
        match action {
            InputAction::SendMessage(input) => {
                self.command_sink
                    .send_command(AppCommand::ProcessUserInput(input))
                    .await?;
                // Clear any selection and reset manual scroll when user sends a message
                self.view_model.message_list_state.selected = None;
                self.view_model.message_list_state.user_scrolled = false;
                debug!(target: "tui.dispatch_input_action", "Sent UserInput command, reset user_scrolled");
            }
            InputAction::ExecuteBashCommand(command) => {
                self.command_sink
                    .send_command(AppCommand::ExecuteBashCommand { command })
                    .await?;
                // Clear any selection and reset manual scroll when user executes a command
                self.view_model.message_list_state.selected = None;
                self.view_model.message_list_state.user_scrolled = false;
                debug!(target: "tui.dispatch_input_action", "Sent ExecuteBashCommand, reset user_scrolled");
            }
            InputAction::ApproveToolNormal(id) => {
                self.command_sink
                    .send_command(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: false,
                    })
                    .await?;
                self.activate_next_approval();
            }
            InputAction::ApproveToolAlways(id) => {
                self.command_sink
                    .send_command(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: true,
                    })
                    .await?;
                self.activate_next_approval();
            }
            InputAction::DenyTool(id) => {
                self.command_sink
                    .send_command(AppCommand::HandleToolResponse {
                        id,
                        approved: false,
                        always: false,
                    })
                    .await?;
                self.activate_next_approval();
            }
            InputAction::CancelProcessing => {
                self.command_sink
                    .send_command(AppCommand::CancelProcessing)
                    .await?;
            }
            InputAction::Exit => {
                should_exit = true;
            }
        }
        Ok(should_exit)
    }

    fn activate_next_approval(&mut self) {
        // Since we clear current_tool_approval after handling, we just return to normal mode
        debug!(target: "tui.activate_next_approval", "Clearing approval state, returning to Normal mode.");
        self.current_tool_approval = None;
        self.input_mode = InputMode::Normal;
    }

    fn clear_textarea(&mut self) {
        self.textarea.delete_line_by_end();
        self.textarea.move_cursor(tui_textarea::CursorMove::Bottom);
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }
}

// Implement Drop to ensure terminal cleanup happens
impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup_terminal() {
            error!(target:"tui.drop", "Failed to cleanup terminal: {}", e);
        }
    }
}

/// Free function for best-effort terminal cleanup (raw mode, alt screen, mouse, etc.)
pub fn cleanup_terminal() {
    use ratatui::crossterm::event::{
        DisableBracketedPaste, DisableMouseCapture, PopKeyboardEnhancementFlags,
    };
    use ratatui::crossterm::{
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

// Helper to wrap terminal cleanup in panic handler
pub fn setup_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
            DisableMouseCapture
        );
        let _ = disable_raw_mode();
        // Print panic info to stderr after restoring terminal state
        eprintln!("Application panicked:");
        eprintln!("{}", panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppCommand;
    use crate::app::conversation::{AssistantContent, Message, ToolResult};
    use crate::app::io::AppCommandSink;
    use crate::tui::widgets::formatters::ToolFormatter;
    use crate::tui::widgets::formatters::view::ViewFormatter;
    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::json;

    // Mock command sink for tests
    struct MockCommandSink;

    #[async_trait]
    impl AppCommandSink for MockCommandSink {
        async fn send_command(&self, _command: AppCommand) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_restore_messages_preserves_tool_call_params() {
        // Create a TUI instance for testing
        let command_sink = Arc::new(MockCommandSink) as Arc<dyn AppCommandSink>;
        let model = crate::api::Model::Claude3_5Sonnet20241022;
        let mut tui = Tui::new(command_sink, model).unwrap();

        // Build test messages: Assistant with ToolCall, then Tool result
        let tool_id = "test_tool_123".to_string();
        let real_params = json!({
            "file_path": "/test/file.rs",
            "offset": 10,
            "limit": 100
        });

        let tool_call = tools::schema::ToolCall {
            id: tool_id.clone(),
            name: "view".to_string(),
            parameters: real_params.clone(),
        };

        let messages = vec![
            Message::Assistant {
                id: "msg_123".to_string(),
                content: vec![AssistantContent::ToolCall { tool_call }],
                timestamp: 1234567890,
            },
            Message::Tool {
                tool_use_id: tool_id.clone(),
                result: ToolResult::Success {
                    output: "file content here".to_string(),
                },
                timestamp: 1234567891,
                id: "tool_result_123".to_string(),
            },
        ];

        // Restore the messages
        tui.restore_messages(messages);

        // Assert we have exactly one Tool message in the view model
        let tool_messages: Vec<_> = tui
            .view_model
            .messages
            .iter()
            .filter_map(|msg| match msg {
                MessageContent::Tool { call, .. } => Some(call),
                _ => None,
            })
            .collect();

        assert_eq!(tool_messages.len(), 1);
        let tool_call = tool_messages[0];

        // Verify the Tool message has proper parameters (not null)
        assert_ne!(tool_call.parameters, serde_json::Value::Null);
        assert_eq!(tool_call.parameters, real_params);
        assert_eq!(tool_call.name, "view");
        assert_eq!(tool_call.id, tool_id);

        // Verify the formatter can parse these parameters without error
        let formatter = ViewFormatter;
        let lines = formatter.compact(&tool_call.parameters, &None, 80);

        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("");

        assert!(!text.contains("Invalid view params"));
        assert!(text.contains("file.rs"));
    }

    #[test]
    fn test_restore_messages_handles_tool_result_before_assistant() {
        // Test edge case where Tool result arrives before Assistant message
        let command_sink = Arc::new(MockCommandSink) as Arc<dyn AppCommandSink>;
        let model = crate::api::Model::Claude3_5Sonnet20241022;
        let mut tui = Tui::new(command_sink, model).unwrap();

        let tool_id = "test_tool_456".to_string();
        let real_params = json!({
            "file_path": "/another/file.rs"
        });

        // Tool result comes first (unusual but possible)
        let messages = vec![
            Message::Tool {
                tool_use_id: tool_id.clone(),
                result: ToolResult::Success {
                    output: "file content".to_string(),
                },
                timestamp: 1234567890,
                id: "tool_result_456".to_string(),
            },
            Message::Assistant {
                id: "msg_456".to_string(),
                content: vec![AssistantContent::ToolCall {
                    tool_call: tools::schema::ToolCall {
                        id: tool_id.clone(),
                        name: "view".to_string(),
                        parameters: real_params.clone(),
                    },
                }],
                timestamp: 1234567891,
            },
        ];

        tui.restore_messages(messages);

        // Should still have proper parameters
        let tool_messages: Vec<_> = tui
            .view_model
            .messages
            .iter()
            .filter_map(|msg| match msg {
                MessageContent::Tool { call, .. } => Some(call),
                _ => None,
            })
            .collect();

        assert_eq!(tool_messages.len(), 1);
        let tool_call = tool_messages[0];
        assert_eq!(tool_call.parameters, real_params);
        assert_eq!(tool_call.name, "view");
    }
}
