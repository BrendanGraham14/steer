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
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::io::{self, Stdout};
use std::panic;
use std::time::{Duration, Instant};
use tui_textarea::{Input, Key, TextArea};

use std::collections::VecDeque;
use tokio::select;

use crate::api::Model;
use crate::app::command::AppCommand;
use crate::app::{AppEvent, conversation::Role};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, error, info, warn};
mod message_formatter;

use message_formatter::{format_message, format_tool_preview, format_tool_result_block};

const MAX_INPUT_HEIGHT: u16 = 10;
const SPINNER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Normal,
    Editing,
    AwaitingApproval,
    ConfirmExit,
}

// Define a struct to hold the necessary info for a pending approval
#[derive(Debug, Clone)]
struct PendingApprovalInfo {
    id: String,
    name: String,
    parameters: serde_json::Value, // Store parameters for display
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    textarea: TextArea<'static>,
    input_mode: InputMode,
    messages: Vec<FormattedMessage>,
    is_processing: bool,
    progress_message: Option<String>,
    last_spinner_update: Instant,
    spinner_state: usize,
    command_tx: mpsc::Sender<AppCommand>,
    pending_tool_approvals: VecDeque<PendingApprovalInfo>, // New queue
    current_tool_approval: Option<PendingApprovalInfo>,    // New current approval state
    scroll_offset: usize,
    max_scroll: usize,
    user_scrolled_away: bool,
    raw_messages: Vec<crate::app::Message>,
    current_model: Model,
}

#[derive(Clone)]
pub struct FormattedMessage {
    content: Vec<Line<'static>>,
    role: Role,
    id: String,
    full_tool_result: Option<String>,
    is_truncated: bool,
}

#[derive(Debug)]
enum InputAction {
    SendMessage(String),
    ToggleMessageTruncation(String),
    ApproveToolNormal(String),
    ApproveToolAlways(String),
    DenyTool(String),
    CancelProcessing,
    Exit,
}

impl Tui {
    pub fn new(command_tx: mpsc::Sender<AppCommand>, initial_model: Model) -> Result<Self> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
            EnableBracketedPaste,
            EnableMouseCapture
        )?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        // Initialize TextArea
        let mut textarea = TextArea::default();
        textarea.set_block(
            ratatui::widgets::Block::default()
                .borders(Borders::ALL)
                .title("Input (Ctrl+S or i to edit, Enter to send, Esc to exit)"),
        );
        textarea.set_placeholder_text("Enter your message here...");
        textarea.set_style(Style::default());

        Ok(Self {
            terminal,
            textarea,
            input_mode: InputMode::Normal,
            messages: Vec::new(),
            is_processing: false,
            progress_message: None,
            last_spinner_update: Instant::now(),
            spinner_state: 0,
            command_tx,
            pending_tool_approvals: VecDeque::new(), // Initialize queue
            current_tool_approval: None,             // Initialize current
            scroll_offset: 0,
            max_scroll: 0,
            user_scrolled_away: false,
            raw_messages: Vec::new(),
            current_model: initial_model,
        })
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
        while !should_exit {
            // Update state needed before drawing (like spinner)
            self.update_spinner_state();

            let input_mode = self.input_mode;
            let is_processing = self.is_processing;
            let progress_message_owned: Option<String> = self.progress_message.clone();
            let spinner_char_owned: String = self.get_spinner_char();
            let current_model_owned: String = self.current_model.to_string();
            let current_tool_approval_info = self.current_tool_approval.clone();
            let pending_tool_approvals_count = self.pending_tool_approvals.len();

            let input_block = Tui::create_input_block_static(
                input_mode,
                is_processing,
                progress_message_owned,
                spinner_char_owned,
                current_tool_approval_info.as_ref(), // Pass current approval info
                pending_tool_approvals_count,        // Pass queue count
            );
            self.textarea.set_block(input_block);

            self.terminal.draw(|f| {
                let textarea_ref = &self.textarea;
                let messages_ref = &self.messages;
                let current_approval_ref = self.current_tool_approval.as_ref(); // Pass reference

                if let Err(e) = Tui::render_ui_static(
                    f,
                    textarea_ref,
                    messages_ref,
                    input_mode,
                    self.scroll_offset,
                    self.max_scroll,
                    &current_model_owned,
                    current_approval_ref, // Pass current approval info for rendering preview
                ) {
                    error!(target:"tui.run.draw", "UI rendering failed: {}", e);
                }
            })?;

            select! {
                // Prioritize terminal events (especially mouse events) for responsiveness
                biased;

                maybe_term_event = term_event_rx.recv() => {
                    match maybe_term_event {
                        Some(Ok(event)) => {
                            match event {
                                Event::Resize(_, _) => {
                                    debug!(target:"tui.run", "Terminal resized");
                                    // Recalculate max_scroll based on new size
                                    let all_lines: Vec<Line> = self.messages.iter().flat_map(|fm| fm.content.clone()).collect();
                                    let total_lines_count = all_lines.len();
                                    let terminal_height = self.terminal.size()?.height;
                                    let input_height = (self.textarea.lines().len() as u16 + 2).min(MAX_INPUT_HEIGHT).min(terminal_height);
                                    let messages_area_height = terminal_height.saturating_sub(input_height).saturating_sub(2); // Account for borders
                                    let new_max_scroll = if total_lines_count > messages_area_height as usize {
                                        total_lines_count.saturating_sub(messages_area_height as usize)
                                    } else {
                                        0
                                    };
                                    self.set_max_scroll(new_max_scroll);
                                    if self.scroll_offset == self.max_scroll {
                                        self.user_scrolled_away = false;
                                    }
                                }
                                Event::Paste(data) => {
                                    if matches!(self.input_mode, InputMode::Editing) {
                                        let normalized_data = data.replace("\\r\\n", "\\n").replace("\\r", "\\n");
                                        self.textarea.insert_str(&normalized_data);
                                        debug!(target:"tui.run", "Pasted {} chars", normalized_data.len());
                                    }
                                }
                                Event::Key(key) => {
                                    match self.handle_input(key).await {
                                        Ok(Some(action)) => {
                                            debug!(target:"tui.run", "Handling input action: {:?}", action);
                                            if self.dispatch_input_action(action).await? {
                                                should_exit = true;
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(e) => {
                                            error!(target:"tui.run",    "Error handling input: {}", e);
                                        }
                                    }
                                }
                                Event::FocusGained => debug!(target:"tui.run", "Focus gained"),
                                Event::FocusLost => debug!(target:"tui.run", "Focus lost"),
                                Event::Mouse(event) => {
                                    // Fast path for mouse events to minimize latency
                                    match event {
                                        MouseEvent {
                                            kind: MouseEventKind::ScrollDown,
                                            ..
                                        } => {
                                            let current_offset = self.get_scroll_offset();
                                            let max_scroll = self.get_max_scroll();
                                            if current_offset < max_scroll {
                                                let new_offset = (current_offset + 20).min(max_scroll);
                                                self.set_scroll_offset(new_offset);
                                                if new_offset == max_scroll {
                                                    self.user_scrolled_away = false; // Reached bottom
                                                }

                                                // Immediate redraw for better responsiveness
                                                if let Err(e) = self.terminal.draw(|f| {
                                                    let textarea_ref = &self.textarea;
                                                    let messages_ref = &self.messages;
                                                    if let Err(e) = Tui::render_ui_static(
                                                        f,
                                                        textarea_ref,
                                                        messages_ref,
                                                        input_mode,
                                                        self.scroll_offset,
                                                        self.max_scroll,
                                                        &current_model_owned,
                                                        self.current_tool_approval.as_ref(), // Pass current approval
                                                    ) {
                                                        error!(target:"tui.run.draw",   "UI rendering failed: {}", e);
                                                    }
                                                }) {
                                                    error!(target:"tui.mouse_scroll", "Failed to redraw: {}", e);
                                                }
                                            }
                                        }
                                        MouseEvent {
                                            kind: MouseEventKind::ScrollUp,
                                            ..
                                        } => {
                                            let current_offset = self.get_scroll_offset();
                                            if current_offset > 0 {
                                                let new_offset = current_offset.saturating_sub(20);
                                                self.set_scroll_offset(new_offset);
                                                self.user_scrolled_away = true; // User scrolled up

                                                // Immediate redraw for better responsiveness
                                                if let Err(e) = self.terminal.draw(|f| {
                                                    let textarea_ref = &self.textarea;
                                                    let messages_ref = &self.messages;
                                                    if let Err(e) = Tui::render_ui_static(
                                                        f,
                                                        textarea_ref,
                                                        messages_ref,
                                                        input_mode,
                                                        self.scroll_offset,
                                                        self.max_scroll,
                                                        &current_model_owned,
                                                        self.current_tool_approval.as_ref(), // Pass current approval
                                                    ) {
                                                        error!(target:"tui.run.draw",   "UI rendering failed: {}", e);
                                                    }
                                                }) {
                                                    error!(target:"tui.mouse_scroll", "Failed to redraw: {}", e);
                                                }
                                            }
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
                        }
                        None => {
                            info!(target:"tui.run", "App event channel closed.");
                            should_exit = true;
                        }
                    }
                }

                _ = tokio::time::sleep(SPINNER_UPDATE_INTERVAL / 2) => {}
            }
        }

        // Cleanup terminal before exiting run loop
        self.cleanup_terminal()?;
        Ok(())
    }

    async fn handle_app_event(&mut self, event: AppEvent) {
        let mut messages_updated = false;
        match event {
            AppEvent::ThinkingStarted => {
                debug!(target:"tui.handle_app_event", "Setting is_processing = true");
                self.is_processing = true;
                self.spinner_state = 0;
                self.progress_message = None;
            }
            AppEvent::ThinkingCompleted | AppEvent::Error { .. } => {
                debug!(target:"tui.handle_app_event", "Setting is_processing = false");
                self.is_processing = false;
                self.progress_message = None;
            }
            AppEvent::MessagePart { id, delta } => {
                debug!(target:"tui.handle_app_event", "MessagePart: {}", id);
                // Find and update the message with the given ID, or create a new one if needed
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                    // Convert the delta to a Line and add it to the content vector
                    let new_line = Line::from(delta.clone());
                    msg.content.push(new_line);
                    messages_updated = true;
                }
            }
            AppEvent::ModelChanged { model } => {
                debug!(target:"tui.handle_app_event", "Model changed to: {}", model);
                self.current_model = model;
            }
            AppEvent::ToolCallStarted { name, id } => {
                self.spinner_state = 0;
                self.progress_message = Some(format!("Executing tool: {}", name));
                debug!(target:"tui.handle_app_event", "Tool call started: {} ({:?})", name, id);

                // Find the corresponding raw message
                let raw_msg = self.raw_messages.iter().find(|m| m.id == id).cloned();

                if let Some(raw_msg) = raw_msg {
                    // Add the raw message to the TUI's internal list
                    self.raw_messages.push(raw_msg.clone());

                    // Format using the actual blocks
                    let formatted = format_message(
                        &raw_msg.content_blocks,
                        Role::Tool,
                        self.terminal.size().map(|r| r.width).unwrap_or(100),
                    );

                    // Check if ID already exists in the TUI's formatted list
                    if self.messages.iter().any(|m| m.id == id) {
                        warn!(target:
                            "tui.handle_app_event",
                            "MessageAdded: ID {} already exists. Skipping.", id,
                        );
                    } else {
                        let formatted_message = FormattedMessage {
                            content: formatted,
                            role: Role::Tool,
                            id: id.clone(),
                            full_tool_result: None,
                            is_truncated: false,
                        };
                        self.messages.push(formatted_message);
                        // self.raw_messages.push(crate::app::Message::new_text(role, content.clone())); // Raw message added above
                        debug!(target: "tui.handle_app_event", "Added message ID: {}", id);
                        messages_updated = true;
                    }
                } else {
                    // This case might happen if the App adds a message but fails to send the event,
                    // or if the event arrives before the App could add it (less likely).
                    warn!(target: "tui.handle_app_event", "MessageAdded event received for ID {}, but corresponding raw message not found yet.", id);
                    // Optionally, add a placeholder formatted message
                    // let placeholder_block = crate::app::conversation::MessageContentBlock::Text("[Message content pending...]".to_string());
                    // let formatted = format_message(&[placeholder_block], role);
                    // ... add formatted message ...
                }
            }
            AppEvent::MessageAdded {
                role,
                content_blocks,
                id,
            } => {
                if role == Role::System {
                    debug!(target: "tui.handle_app_event", "Skipping system message display");
                    // Still add to raw messages if needed for context/history?
                    // self.raw_messages.push(crate::app::Message::new_with_blocks(role, content_blocks));
                    return;
                }

                // MessageAdded now carries the blocks directly
                // Add the raw message first
                let raw_msg = crate::app::Message::new_with_blocks(role, content_blocks.clone());
                self.raw_messages.push(raw_msg.clone());

                // Format using the actual blocks
                let formatted = format_message(
                    &content_blocks,
                    role,
                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                );

                // Check if ID already exists in the TUI's formatted list
                // (Should ideally not happen with unique IDs, but good safety check)
                if self.messages.iter().any(|m| m.id == id) {
                    info!(
                        target: "tui.handle_app_event",
                        "MessageAdded: ID {} already exists. Content blocks: {}. Skipping.",
                        id,
                        content_blocks.len()
                    );
                } else {
                    let formatted_message = FormattedMessage {
                        content: formatted,
                        role,
                        id: id.clone(),
                        full_tool_result: None,
                        is_truncated: false,
                    };
                    self.messages.push(formatted_message);
                    messages_updated = true;
                    debug!(
                        target: "tui.handle_app_event",
                        "Added message ID: {} with {} content blocks",
                        id,
                        content_blocks.len()
                    );
                }
            }
            AppEvent::MessageUpdated { id, content } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                    debug!(
                        target: "tui.handle_app_event",
                        "Updating message ID: {}", id,
                    );
                    // Now use the blocks from the raw message
                    if let Some(raw_msg) = self.raw_messages.iter().find(|m| m.id == id) {
                        msg.content = format_message(
                            &raw_msg.content_blocks,
                            msg.role,
                            self.terminal.size().map(|r| r.width).unwrap_or(100),
                        );
                        messages_updated = true; // Mark that message content changed
                        debug!(
                            target: "tui.handle_app_event",
                            "Updated message ID: {} with new blocks", id,
                        );
                    } else {
                        warn!(target: "tui.handle_app_event", "MessageUpdated: Raw message ID {} not found.", id);
                        // Fallback: format the string content as a text block
                        let block = crate::app::conversation::MessageContentBlock::Text(content);
                        msg.content = format_message(
                            &[block],
                            msg.role,
                            self.terminal.size().map(|r| r.width).unwrap_or(100),
                        );
                        messages_updated = true; // Mark that message content changed
                    }
                } else {
                    warn!(target: "tui.handle_app_event", "MessageUpdated: ID {} not found.", id);
                }
            }
            AppEvent::ToolCallCompleted {
                name: _, // Name is implicitly shown in the formatted result block
                result,
                id, // Use this ID for the formatted message
            } => {
                // If the completed tool matches the *currently* displayed approval,
                // clear the current approval state and activate the next one.
                // This handles cases where a tool might execute automatically (e.g., always approved)
                // while another was pending user input.
                if let Some(current_approval) = &self.current_tool_approval {
                    if current_approval.id == id {
                        debug!(target: "tui.handle_app_event", "Tool {} (which was pending approval) completed. Activating next.", id);
                        self.current_tool_approval = None; // Clear current explicitly
                    // self.activate_next_approval();
                    } else {
                        // A different tool completed, progress message was likely set by ToolCallStarted
                        self.progress_message = None;
                    }
                } else {
                    // No tool was awaiting approval, just clear progress
                    self.progress_message = None;
                }

                debug!(target: "tui.handle_app_event", "Adding Tool Result message for ID: {}", id);

                let formatted_result_lines = format_tool_result_block(
                    &id,
                    &result,
                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                );

                // Check if result should be truncated
                const MAX_PREVIEW_LINES: usize = 5;
                let lines: Vec<&str> = result.lines().collect();
                let should_truncate = lines.len() > MAX_PREVIEW_LINES;

                let content = if should_truncate {
                    // Create truncated preview content
                    let preview_content = format!(
                        "{}\n... ({} more lines, press 'Ctrl+r' (in normal mode) to toggle full view)",
                        lines
                            .iter()
                            .take(MAX_PREVIEW_LINES)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n"),
                        lines.len() - MAX_PREVIEW_LINES
                    );
                    format_tool_preview(
                        &preview_content,
                        self.terminal.size().map(|r| r.width).unwrap_or(100),
                    )
                } else {
                    // Use original formatting if not truncated
                    formatted_result_lines
                };

                let formatted_message = FormattedMessage {
                    content,
                    role: Role::Tool,
                    id: format!("result_{}", id), // Ensure unique ID for the result display
                    full_tool_result: Some(result),
                    is_truncated: should_truncate,
                };
                self.messages.push(formatted_message);
                messages_updated = true;
            }
            AppEvent::ToolCallFailed { name, error, id } => {
                self.progress_message = None; // Clear progress on failure

                // Similar logic to ToolCallCompleted: Assuming this relates to a Role::Tool message
                // added via MessageAdded.
                debug!(target: "tui.handle_app_event", "Adding Tool Failure message for ID: {}, Error: {}", id, error);

                // Create a NEW message to display the tool failure
                let failure_content = format!("Tool '{}' failed: {}.", name, error);
                let formatted_failure_lines = vec![Line::from(Span::styled(
                    failure_content,
                    Style::default().fg(Color::Red),
                ))];

                let formatted_message = FormattedMessage {
                    content: formatted_failure_lines,
                    role: Role::Tool,
                    id: format!("failed_{}", id), // Ensure unique ID
                    full_tool_result: Some(format!("Error: {}", error)), // Store error for potential toggle?
                    is_truncated: false,
                };
                self.messages.push(formatted_message);
                messages_updated = true;
            }
            AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
            } => {
                // Store approval request state in the queue
                let approval_info = PendingApprovalInfo {
                    id: id.clone(),
                    name: name.clone(),
                    parameters,
                };
                self.pending_tool_approvals.push_back(approval_info);
                debug!(target: "tui.handle_app_event", "Queued tool approval request for {} ({}). Pending approvals: {}", name, id, self.pending_tool_approvals.len());

                // If no approval is currently active, activate this one
                if self.current_tool_approval.is_none() {
                    self.activate_next_approval();
                }
            }
            AppEvent::CommandResponse { content, id: _ } => {
                let response_id = format!("cmd_resp_{}", chrono::Utc::now().timestamp_millis());
                let block = crate::app::conversation::MessageContentBlock::Text(content.clone()); // Clone content
                let formatted = format_message(
                    &[block],
                    Role::System,
                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                );
                self.raw_messages
                    .push(crate::app::Message::new_text(Role::System, content));
                self.messages.push(FormattedMessage {
                    content: formatted,
                    role: Role::System,
                    id: response_id,
                    full_tool_result: None,
                    is_truncated: false,
                });
                messages_updated = true;
            }
            AppEvent::OperationCancelled { info } => {
                self.is_processing = false;
                self.progress_message = None;
                // Clear the queue and the current approval on cancellation
                self.pending_tool_approvals.clear();
                self.current_tool_approval = None;
                self.input_mode = InputMode::Normal;
                self.spinner_state = 0;
                // Use the Display impl of CancellationInfo directly
                let cancellation_text = format!("Operation cancelled: {}", info);
                self.raw_messages.push(crate::app::Message::new_text(
                    Role::User, // Consider Role::System?
                    cancellation_text.clone(),
                ));
                self.messages.push(FormattedMessage {
                    content: vec![Line::from(Span::styled(
                        cancellation_text,
                        Style::default().fg(Color::Red),
                    ))],
                    role: Role::User,
                    id: format!("cancellation_{}", chrono::Utc::now().timestamp_millis()),
                    full_tool_result: None,
                    is_truncated: false,
                });
                messages_updated = true;
            }
        }

        // --- Handle Scrolling After Message Updates ---
        if messages_updated {
            // Recalculate max scroll based on the *potentially updated* message list
            if let Ok(term_size) = self.terminal.size() {
                let all_lines: Vec<Line> = self
                    .messages
                    .iter()
                    .flat_map(|fm| fm.content.clone())
                    .collect();
                let total_lines_count = all_lines.len();
                let terminal_height = term_size.height;
                let input_height = (self.textarea.lines().len() as u16 + 2)
                    .min(MAX_INPUT_HEIGHT)
                    .min(terminal_height);
                let messages_area_height = terminal_height
                    .saturating_sub(input_height)
                    .saturating_sub(2);
                let new_max_scroll = if total_lines_count > messages_area_height as usize {
                    total_lines_count.saturating_sub(messages_area_height as usize)
                } else {
                    0
                };
                self.set_max_scroll(new_max_scroll);

                // Scroll to bottom ONLY if user wasn't scrolled away
                if !self.user_scrolled_away {
                    debug!(target: "tui.handle_app_event", "Scrolling to bottom after message update.");
                    self.set_scroll_offset(self.max_scroll);
                } else {
                    debug!(target: "tui.handle_app_event", "User scrolled away, not scrolling to bottom.");
                    // If the update caused the current offset to become the max, reset the flag
                    if self.scroll_offset == self.max_scroll {
                        self.user_scrolled_away = false;
                    }
                }
            } else {
                warn!(target: "tui.handle_app_event", "Failed to get terminal size for scroll update.");
            }
        }
    }

    fn get_scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn set_scroll_offset(&mut self, offset: usize) {
        let clamped_offset = offset.min(self.max_scroll);
        self.scroll_offset = clamped_offset;
    }

    fn get_max_scroll(&self) -> usize {
        self.max_scroll
    }

    fn set_max_scroll(&mut self, max: usize) {
        self.max_scroll = max;
        if self.scroll_offset > self.max_scroll {
            self.scroll_offset = self.max_scroll;
        }
    }

    fn get_spinner_char(&self) -> String {
        const SPINNER: [&str; 4] = ["⠋", "⠙", "⠹", "⠸"];
        SPINNER[self.spinner_state % SPINNER.len()].to_string()
    }

    fn update_spinner_state(&mut self) {
        if self.last_spinner_update.elapsed() > SPINNER_UPDATE_INTERVAL {
            self.spinner_state = self.spinner_state.wrapping_add(1);
            self.last_spinner_update = Instant::now();
        }
    }

    fn create_input_block_static<'a>(
        input_mode: InputMode,
        is_processing: bool,
        progress_message: Option<String>,
        spinner_char: String,
        current_tool_approval_info: Option<&PendingApprovalInfo>, // New arg
        pending_tool_approvals_count: usize,                      // New arg
    ) -> Block<'a> {
        let input_border_style = match input_mode {
            InputMode::Editing => Style::default().fg(Color::Yellow),
            InputMode::Normal => Style::default().fg(Color::DarkGray),
            InputMode::AwaitingApproval => {
                // Only style as ApprovalRequired if there *is* a current approval
                if current_tool_approval_info.is_some() {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray) // Or Normal style if somehow in mode without approval
                }
            }
            InputMode::ConfirmExit => Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        };
        let input_title: String = match input_mode {
            InputMode::Editing => "Input (Esc to stop editing, Enter to send)".to_string(),
            InputMode::Normal => "Input (i to edit, Enter to send, Ctrl+C to exit)".to_string(),
            InputMode::AwaitingApproval => {
                // Update title based on current approval info
                if let Some(info) = current_tool_approval_info {
                    let queue_count_str = if pending_tool_approvals_count > 0 {
                        format!(" [{} more queued]", pending_tool_approvals_count)
                    } else {
                        String::new()
                    };
                    format!(
                        "Approve Tool: '{}'? (y/n, Shift+Tab=always, Esc=deny){}",
                        info.name, queue_count_str
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
        messages: &[FormattedMessage],
        input_mode: InputMode,
        scroll_offset: usize,
        max_scroll: usize,
        current_model: &str,
        current_approval_info: Option<&PendingApprovalInfo>, // New arg for preview
    ) -> Result<()> {
        let total_area = f.area();
        let input_height = (textarea.lines().len() as u16 + 2)
            .min(MAX_INPUT_HEIGHT)
            .min(total_area.height);

        // Calculate potential preview height
        let preview_height = if current_approval_info.is_some() {
            5 // Example fixed height, adjust as needed
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

        // Render Messages
        Self::render_messages_static(f, messages_area, messages, scroll_offset, max_scroll);

        // Render Tool Preview (conditionally)
        if let Some(info) = current_approval_info {
            Self::render_tool_preview_static(f, preview_area, info);
        }

        // Render Text Area
        f.render_widget(textarea, input_area);

        // Render model info at the bottom
        let model_info: Paragraph<'_> = Paragraph::new(Line::from(Span::styled(
            current_model,
            Style::default().fg(Color::LightMagenta),
        )))
        .alignment(ratatui::layout::Alignment::Right);
        f.render_widget(model_info, model_info_area);

        // Set cursor visibility and position
        if input_mode == InputMode::Editing {
            // Calculate cursor position relative to the input area
            let cursor_col = input_area.x + 1 + textarea.cursor().0 as u16;
            let cursor_row = input_area.y + 1 + textarea.cursor().1 as u16;

            // Ensure cursor stays within the visible area of the textarea block
            if cursor_row < input_area.bottom() - 1 && cursor_col < input_area.right() - 1 {
                // Use set_cursor_position with a Position struct
                f.set_cursor_position(Position {
                    x: cursor_col,
                    y: cursor_row,
                });
            } else {
                // Hide cursor if it would be outside the box (e.g., during scrolling)
                f.set_cursor_position(Position { x: 0, y: 0 });
            }
        }

        Ok(())
    }

    fn render_messages_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        messages: &[FormattedMessage],
        scroll_offset: usize,
        max_scroll: usize,
    ) {
        if messages.is_empty() {
            let placeholder = Paragraph::new("No messages yet...")
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false });
            f.render_widget(placeholder, area);
            return;
        }

        // Flatten message content lines for calculating total height and rendering
        let all_lines: Vec<Line> = messages.iter().flat_map(|fm| fm.content.clone()).collect();
        let total_lines_count = all_lines.len();

        let area_height = area.height.saturating_sub(2) as usize;

        // Create list items from the flattened lines, applying scroll offset manually via slicing
        let start = scroll_offset.min(total_lines_count.saturating_sub(1));
        let end = (scroll_offset + area_height).min(total_lines_count);
        let visible_items: Vec<ListItem> = if start < end {
            all_lines[start..end]
                .iter()
                .cloned()
                .map(ListItem::new)
                .collect()
        } else {
            Vec::new()
        };

        let messages_list = List::new(visible_items)
            .block(Block::default().borders(Borders::ALL).title("Conversation"));

        f.render_widget(messages_list, area);

        // Render Scrollbar
        if max_scroll > 0 {
            let scrollbar = ratatui::widgets::Scrollbar::new(
                ratatui::widgets::ScrollbarOrientation::VerticalRight,
            );
            // Adjust scrollbar area to be inside the message block borders
            let scrollbar_area = area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            });

            let mut scrollbar_state =
                ratatui::widgets::ScrollbarState::new(total_lines_count).position(scroll_offset);

            f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
        }
    }

    // New static function to render the tool preview
    fn render_tool_preview_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        info: &PendingApprovalInfo,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Tool Preview: {} (ID: {}) ", info.name, info.id))
            .border_style(Style::default().fg(Color::Cyan));

        // Format parameters nicely (e.g., pretty JSON)
        let params_str = serde_json::to_string_pretty(&info.parameters)
            .unwrap_or_else(|_| format!("Error formatting parameters: {:?}", info.parameters));

        let text = Paragraph::new(params_str)
            .block(block)
            .wrap(Wrap { trim: true });

        f.render_widget(text, area);
    }

    async fn handle_input(&mut self, key: KeyEvent) -> Result<Option<InputAction>> {
        let mut action = None;
        let half_page_height = self
            .terminal
            .size()?
            .height
            .saturating_sub(self.textarea.lines().len() as u16 + 2)
            .saturating_sub(2)
            .saturating_div(2) as usize;

        let full_page_height = half_page_height * 2;
        // --- Global keys handled first ---

        // Handle ESC specifically for denying the current tool approval
        if key.code == KeyCode::Esc && self.current_tool_approval.is_some() {
            if let Some(approval_info) = self.current_tool_approval.take() {
                action = Some(InputAction::DenyTool(approval_info.id));
                // self.activate_next_approval(); // Activate next after taking action
                return Ok(action);
            }
        }

        // --- Scrolling and other non-mode-specific keys ---
        // Check if we are *not* in editing mode for scroll keys etc.
        if self.input_mode != InputMode::Editing {
            match key.code {
                // Full Page Scroll Up
                KeyCode::PageUp => {
                    let current_offset = self.get_scroll_offset();
                    let new_offset = current_offset.saturating_sub(full_page_height);
                    if new_offset < current_offset {
                        // Check if scroll actually happened
                        self.set_scroll_offset(new_offset);
                        self.user_scrolled_away = true; // User scrolled up
                    }
                    return Ok(None); // Just scroll
                }
                // Full Page Scroll Down
                KeyCode::PageDown => {
                    let current_offset = self.get_scroll_offset();
                    let max_scroll = self.get_max_scroll();
                    let new_offset = (current_offset + full_page_height).min(max_scroll);
                    if new_offset > current_offset {
                        // Check if scroll actually happened
                        self.set_scroll_offset(new_offset);
                        if new_offset == max_scroll {
                            self.user_scrolled_away = false; // Reached bottom
                        }
                    }
                    return Ok(None); // Just scroll
                }

                // Line Scroll Up (Arrows)
                KeyCode::Up => {
                    let current_offset = self.get_scroll_offset();
                    if current_offset > 0 {
                        self.set_scroll_offset(current_offset - 1);
                        self.user_scrolled_away = true; // User scrolled up
                    }
                    return Ok(None); // Just scroll
                }
                // Line Scroll Down (Arrows)
                KeyCode::Down => {
                    let current_offset = self.get_scroll_offset();
                    let max_scroll = self.get_max_scroll();
                    if current_offset < max_scroll {
                        let new_offset = current_offset + 1;
                        self.set_scroll_offset(new_offset);
                        if new_offset == max_scroll {
                            self.user_scrolled_away = false; // Reached bottom
                        }
                    }
                    return Ok(None); // Just scroll
                }

                // Line Scroll Up (k in Normal Mode only)
                KeyCode::Char('k') if self.input_mode == InputMode::Normal => {
                    let current_offset = self.get_scroll_offset();
                    if current_offset > 0 {
                        self.set_scroll_offset(current_offset - 1);
                        self.user_scrolled_away = true; // User scrolled up
                    }
                    return Ok(None); // Just scroll
                }
                // Line Scroll Down (j in Normal Mode only)
                KeyCode::Char('j') if self.input_mode == InputMode::Normal => {
                    let current_offset = self.get_scroll_offset();
                    let max_scroll = self.get_max_scroll();
                    if current_offset < max_scroll {
                        let new_offset = current_offset + 1;
                        self.set_scroll_offset(new_offset);
                        if new_offset == max_scroll {
                            self.user_scrolled_away = false; // Reached bottom
                        }
                    }
                    return Ok(None); // Just scroll
                }

                // Half Page Scroll Up (u/Ctrl+u in Normal Mode only)
                KeyCode::Char('u') if self.input_mode == InputMode::Normal => {
                    if key.modifiers == KeyModifiers::CONTROL || key.modifiers == KeyModifiers::NONE
                    {
                        let current_offset = self.get_scroll_offset();
                        let new_offset = current_offset.saturating_sub(half_page_height);
                        if new_offset < current_offset {
                            // Check if scroll actually happened
                            self.set_scroll_offset(new_offset);
                            self.user_scrolled_away = true; // User scrolled up
                        }
                        return Ok(None); // Just scroll
                    }
                }
                // Half Page Scroll Down (d/Ctrl+d in Normal Mode only)
                KeyCode::Char('d')
                    if (self.input_mode == InputMode::Normal
                        && key.modifiers == KeyModifiers::NONE) =>
                {
                    let current_offset = self.get_scroll_offset();
                    let max_scroll = self.get_max_scroll();
                    let new_offset = (current_offset + half_page_height).min(max_scroll);
                    if new_offset > current_offset {
                        // Check if scroll actually happened
                        self.set_scroll_offset(new_offset);
                        if new_offset == max_scroll {
                            self.user_scrolled_away = false; // Reached bottom
                        }
                    }
                    return Ok(None); // Just scroll
                }

                // Toggle Tool Result Truncation (Ctrl+R)
                KeyCode::Char('r') | KeyCode::Char('R')
                    if key.modifiers == KeyModifiers::CONTROL =>
                {
                    let scroll_offset = self.get_scroll_offset();
                    // Find the message corresponding to the current view port top
                    // This is an approximation - ideally we'd track selected message
                    let mut line_count = 0;
                    let mut target_message_id = None;
                    for msg in &self.messages {
                        let msg_lines = msg.content.len();
                        if line_count + msg_lines > scroll_offset {
                            if msg.role == Role::Tool && msg.full_tool_result.is_some() {
                                target_message_id = Some(msg.id.clone());
                            }
                            break; // Found the message at the current scroll offset
                        }
                        line_count += msg_lines;
                    }

                    if let Some(id) = target_message_id {
                        action = Some(InputAction::ToggleMessageTruncation(id));
                    } else {
                        // Maybe check the last message if no specific one found?
                        if let Some(last_tool_msg) = self
                            .messages
                            .iter()
                            .rev()
                            .find(|m| m.role == Role::Tool && m.full_tool_result.is_some())
                        {
                            action = Some(InputAction::ToggleMessageTruncation(
                                last_tool_msg.id.clone(),
                            ));
                        } else {
                            debug!(target: "tui.handle_input", "No tool message found to toggle truncation");
                        }
                    }
                }
                _ => {} // Other keys ignored in this block
            }
        }

        // --- Ctrl+C / Ctrl+D Exit Handling ---
        // This remains mostly the same, might need slight adjustment based on approval mode presence
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            match self.input_mode {
                InputMode::ConfirmExit => {
                    return Ok(Some(InputAction::Exit));
                }
                _ => {
                    debug!(target: "tui.handle_input", "Ctrl+C detected, entering ConfirmExit mode.");
                    self.input_mode = InputMode::ConfirmExit;
                    return Ok(None); // Change mode, no immediate action
                }
            }
        }

        // --- Mode-specific handling OR Current Approval Handling ---

        // Handle Approval Keys if a tool approval is currently active
        if let Some(approval_info) = &self.current_tool_approval {
            // Clone the ID now because we might take() the approval later
            let current_approval_id = approval_info.id.clone();
            let mut approval_action_taken = false;

            match key.code {
                // Approve Normal
                KeyCode::Char('y') | KeyCode::Char('Y') if key.modifiers == KeyModifiers::NONE => {
                    action = Some(InputAction::ApproveToolNormal(current_approval_id));
                    approval_action_taken = true;
                }
                // Approve Always (Shift + Tab)
                KeyCode::BackTab if key.modifiers == KeyModifiers::SHIFT => {
                    action = Some(InputAction::ApproveToolAlways(current_approval_id));
                    approval_action_taken = true;
                }
                // Deny (n/N) - Esc is handled earlier
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    action = Some(InputAction::DenyTool(current_approval_id));
                    approval_action_taken = true;
                }
                _ => {} // Other keys ignored while waiting for approval
            }

            if approval_action_taken {
                self.current_tool_approval.take(); // Consume the current approval
                // Activate next approval after sending the command
                // self.activate_next_approval();
                return Ok(action);
            }
            // If no approval action was taken, prevent other mode-specific actions
            // while waiting for approval, except for global keys handled above.
            return Ok(None);
        }

        // --- Original Mode-Specific Handling (Only if NOT waiting for approval) ---
        // Existing logic for Editing, Normal, ConfirmExit modes
        match self.input_mode {
            InputMode::Editing => {
                match key.into() {
                    // Send message on Enter (unless Shift+Enter for newline)
                    Input {
                        key: Key::Enter,
                        ctrl: false,
                        alt: false,
                        shift: false, // Explicitly check shift is NOT pressed
                    } => {
                        let current_input = self.textarea.lines().join("\n"); // Use newline char
                        if !current_input.trim().is_empty() {
                            action = Some(InputAction::SendMessage(current_input));
                            // Reset textarea fully after sending
                            let mut new_textarea = TextArea::default();
                            new_textarea
                                .set_block(self.textarea.block().cloned().unwrap_or_default());
                            new_textarea.set_placeholder_text(self.textarea.placeholder_text());
                            new_textarea.set_style(self.textarea.style());
                            self.textarea = new_textarea;
                            self.input_mode = InputMode::Normal; // Switch back after sending
                        } else {
                            self.input_mode = InputMode::Normal; // Switch back even if empty
                        }
                    }
                    // Stop editing on Esc
                    Input { key: Key::Esc, .. } => {
                        self.input_mode = InputMode::Normal;
                    }
                    // Default: Pass key to textarea
                    input => {
                        // Only pass input if no action was already determined (like scrolling)
                        if action.is_none() {
                            self.textarea.input(input);
                        }
                    }
                }
            }
            InputMode::Normal => {
                // Only handle non-scrolling keys here if no action already set
                if action.is_none() {
                    match key.code {
                        // Start editing
                        KeyCode::Char('i') | KeyCode::Char('I') => {
                            self.input_mode = InputMode::Editing;
                        }
                        // Send message directly if Enter is pressed in Normal mode
                        KeyCode::Enter => {
                            let current_input = self.textarea.lines().join("\n"); // Use newline char
                            if !current_input.trim().is_empty() {
                                action = Some(InputAction::SendMessage(current_input));
                                // Reset textarea fully after sending
                                let mut new_textarea = TextArea::default();
                                new_textarea
                                    .set_block(self.textarea.block().cloned().unwrap_or_default());
                                new_textarea.set_placeholder_text(self.textarea.placeholder_text());
                                new_textarea.set_style(self.textarea.style());
                                self.textarea = new_textarea;
                            }
                        }
                        // Cancel processing on Esc in Normal mode
                        KeyCode::Esc => {
                            action = Some(InputAction::CancelProcessing);
                        }
                        // Toggle Tool Result Truncation (Ctrl+R)
                        KeyCode::Char('r') | KeyCode::Char('R')
                            if key.modifiers == KeyModifiers::CONTROL =>
                        {
                            let scroll_offset = self.get_scroll_offset();
                            // Find the message corresponding to the current view port top
                            // This is an approximation - ideally we'd track selected message
                            let mut line_count = 0;
                            let mut target_message_id = None;
                            for msg in &self.messages {
                                let msg_lines = msg.content.len();
                                if line_count + msg_lines > scroll_offset {
                                    if msg.role == Role::Tool && msg.full_tool_result.is_some() {
                                        target_message_id = Some(msg.id.clone());
                                    }
                                    break; // Found the message at the current scroll offset
                                }
                                line_count += msg_lines;
                            }

                            if let Some(id) = target_message_id {
                                action = Some(InputAction::ToggleMessageTruncation(id));
                            } else {
                                // Maybe check the last message if no specific one found?
                                if let Some(last_tool_msg) =
                                    self.messages.iter().rev().find(|m| {
                                        m.role == Role::Tool && m.full_tool_result.is_some()
                                    })
                                {
                                    action = Some(InputAction::ToggleMessageTruncation(
                                        last_tool_msg.id.clone(),
                                    ));
                                } else {
                                    debug!(target: "tui.handle_input", "No tool message found to toggle truncation");
                                }
                            }
                        }
                        _ => {} // Other keys ignored in Normal mode unless handled globally/as scrolling
                    }
                }
            }
            InputMode::AwaitingApproval => {
                // This mode is now primarily handled by the `if let Some(approval_info) = &self.current_tool_approval` block above.
                // We shouldn't reach here if an approval is active.
                warn!(target:"tui.handle_input", "In AwaitingApproval mode but current_tool_approval is None. Resetting to Normal.");
                self.input_mode = InputMode::Normal;
            }
            InputMode::ConfirmExit => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        action = Some(InputAction::Exit); // Confirm exit
                    }
                    KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                        action = Some(InputAction::Exit);
                    }
                    _ => {
                        // Any other key cancels
                        self.input_mode = InputMode::Normal; // Go back to normal mode
                    }
                }
            }
        }

        Ok(action)
    }

    // Dispatch action, sending commands if necessary
    // Returns Ok(true) if the app should exit, Ok(false) otherwise
    async fn dispatch_input_action(&mut self, action: InputAction) -> Result<bool> {
        match action {
            InputAction::SendMessage(message) => {
                if message.starts_with('/') {
                    // Send as command
                    self.command_tx
                        .send(AppCommand::ExecuteCommand(message))
                        .await?;
                } else {
                    // Now send command to App to process this *already added* message
                    self.command_tx
                        .send(AppCommand::ProcessUserInput(message))
                        .await?;
                    // Let the App handle adding the message and sending MessageAdded event
                    // The TUI will display the message when it receives that event
                }
            }
            InputAction::ToggleMessageTruncation(id) => {
                // Directly modify the message state here
                let mut found_and_toggled = false;
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                    if let Some(full_result) = &msg.full_tool_result {
                        msg.is_truncated = !msg.is_truncated;

                        if msg.is_truncated {
                            const MAX_PREVIEW_LINES: usize = 5;
                            let lines: Vec<&str> = full_result.lines().collect();
                            let preview_content = if lines.len() > MAX_PREVIEW_LINES {
                                format!(
                                    "{}\n... ({} more lines, press 'Ctrl+r' (in normal mode) to toggle full view)",
                                    lines
                                        .iter()
                                        .take(MAX_PREVIEW_LINES)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    lines.len() - MAX_PREVIEW_LINES
                                )
                            } else {
                                full_result.clone()
                            };
                            msg.content = format_tool_preview(
                                &preview_content,
                                self.terminal.size().map(|r| r.width).unwrap_or(100),
                            );
                        } else {
                            msg.content = format_tool_preview(
                                full_result,
                                self.terminal.size().map(|r| r.width).unwrap_or(100),
                            );
                        }
                        found_and_toggled = true;
                    } else {
                        warn!(target:
                            "tui.dispatch",
                                "Message {} found, but has no full_tool_result to toggle.",
                                id
                        );
                    }
                }
                if !found_and_toggled {
                    warn!(target:
                        "tui.dispatch",
                        "ToggleMessageTruncation: No message found with ID {}", id,
                    );
                }
                // No command needs to be sent to App for this purely visual toggle
            }
            InputAction::ApproveToolNormal(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: false,
                    })
                    .await?;
                // Activate next approval after sending the command
                self.activate_next_approval();
            }
            InputAction::ApproveToolAlways(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: true,
                    })
                    .await?;
                // Activate next approval after sending the command
                self.activate_next_approval();
            }
            InputAction::DenyTool(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: false,
                        always: false,
                    })
                    .await?;
                // Activate next approval after sending the command
                self.activate_next_approval();
            }
            InputAction::CancelProcessing => {
                self.command_tx.send(AppCommand::CancelProcessing).await?;
            }
            InputAction::Exit => {
                // Signal exit cleanly if possible
                if let Err(e) = self.command_tx.send(AppCommand::Shutdown).await {
                    error!(target:
                        "tui.dispatch",
                        "Failed to send Shutdown command: {}", e
                    );
                    // Still exit the TUI loop even if shutdown command fails
                }
                return Ok(true); // Signal exit
            }
        }
        Ok(false) // Don't exit by default
    }

    // Helper function to activate the next approval from the queue
    fn activate_next_approval(&mut self) {
        if let Some(next_approval) = self.pending_tool_approvals.pop_front() {
            debug!(target: "tui.activate_next", "Activating approval for {} ({}). Pending approvals: {}", next_approval.name, next_approval.id, self.pending_tool_approvals.len());
            self.current_tool_approval = Some(next_approval);
            self.input_mode = InputMode::AwaitingApproval;
            // Set progress message for the active approval
            self.progress_message = self
                .current_tool_approval
                .as_ref()
                .map(|info| format!("Tool: {} - Approval Required", info.name));
            self.is_processing = true; // Ensure spinner is active
        } else {
            debug!(target: "tui.activate_next", "No more pending approvals.");
            self.current_tool_approval = None;
            self.input_mode = InputMode::Normal;
            // Clear progress message only if we are not otherwise processing
            if !self.is_processing {
                self.progress_message = None;
            }
            // Don't set is_processing = false here, other background tasks might still be running
        }
    }
}

// Implement Drop to ensure terminal cleanup happens
impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup_terminal() {
            // Log error if cleanup fails, but don't panic in drop
            eprintln!("Failed to cleanup terminal: {}", e);
            error!(target:"Tui::drop", "Failed to cleanup terminal: {}", e);
        }
    }
}

pub async fn run_tui(
    command_tx: mpsc::Sender<AppCommand>,
    event_rx: mpsc::Receiver<AppEvent>,
    initial_model: Model,
) -> Result<()> {
    // Set up panic hook to ensure terminal is reset if the app crashes
    // Clone command_tx for potential use in panic hook if needed later
    let _command_tx_panic = command_tx.clone();
    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // Attempt to clean up the terminal
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);

        error!(target: "panic_hook", "Application panicked: {}", panic_info);

        eprintln!("\nERROR: Application crashed: {}", panic_info);

        // Call the original panic hook
        orig_hook(panic_info);
    }));

    // --- TUI Initialization ---
    info!(target: "tui::run_tui", "Initializing TUI");
    let mut tui = match Tui::new(command_tx.clone(), initial_model) {
        Ok(tui) => tui,
        Err(e) => {
            error!(target: "tui::run_tui", "Failed to initialize TUI: {}", e);
            // We might be mid-panic hook here, but try to print
            eprintln!("Error: Failed to initialize terminal UI: {}", e);
            return Err(e);
        }
    };

    // --- Run the TUI Loop ---
    info!(target: "tui::run_tui", "Starting TUI run loop");
    let tui_result = tui.run(event_rx).await;
    info!(target: "tui::run_tui", "TUI run loop finished");

    // Handle TUI result
    match tui_result {
        Ok(_) => {
            info!(target: "tui::run_tui", "TUI terminated normally");
            Ok(())
        }
        Err(e) => {
            error!(target: "tui::run_tui", "TUI task error: {}", e);
            eprintln!("Error in TUI: {}", e);
            Err(e)
        }
    }
}
