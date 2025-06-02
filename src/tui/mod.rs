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

use message_formatter::{format_command_response, format_message, format_tool_preview};

const MAX_INPUT_HEIGHT: u16 = 10;
const SPINNER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Normal,
    Editing,
    AwaitingApproval,
    ConfirmExit,
}

// Define scroll direction and amount
#[derive(Debug, Clone, Copy, PartialEq)]
enum ScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ScrollAmount {
    Line(usize),
    HalfPage,
    FullPage,
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
    display_items: Vec<DisplayItem>,
    is_processing: bool,
    progress_message: Option<String>,
    last_spinner_update: Instant,
    spinner_state: usize,
    command_tx: mpsc::Sender<AppCommand>,
    current_tool_approval: Option<PendingApprovalInfo>,
    scroll_offset: usize,
    max_scroll: usize,
    user_scrolled_away: bool,
    raw_messages: Vec<crate::app::Message>,
    current_model: Model,
}

// Represents different types of items displayed in the TUI conversation list.
#[derive(Clone, Debug)]
enum DisplayItem {
    Message {
        id: String,
        role: Role, // Keep Role for User/Assistant/Tool messages
        content: Vec<Line<'static>>,
    },
    ToolResult {
        id: String,                  // Unique ID for this display item (e.g., "result_tool_abc")
        tool_use_id: String,         // ID of the original tool call
        content: Vec<Line<'static>>, // Formatted lines (potentially truncated)
        full_result: String,         // Unformatted, full result
        is_truncated: bool,
    },
    CommandResponse {
        id: String,
        content: Vec<Line<'static>>,
    },
    SystemInfo {
        id: String,
        content: Vec<Line<'static>>, // For errors, cancellations, info messages
    },
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
            display_items: Vec::new(),
            is_processing: false,
            progress_message: None,
            last_spinner_update: Instant::now(),
            spinner_state: 0,
            command_tx,
            current_tool_approval: None,
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
        // Request current conversation state to populate display with any existing messages
        // (e.g., when resuming a session)
        if let Err(e) = self
            .command_tx
            .send(AppCommand::GetCurrentConversation)
            .await
        {
            warn!(target:"tui.run", "Failed to request current conversation: {}", e);
        }

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
                let display_items_ref = &self.display_items;
                let current_approval_ref = self.current_tool_approval.as_ref();

                if let Err(e) = Tui::render_ui_static(
                    f,
                    textarea_ref,
                    display_items_ref,
                    input_mode,
                    self.scroll_offset,
                    self.max_scroll,
                    &current_model_owned,
                    current_approval_ref,
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
                                    // Recalculate max_scroll based on new size and adjust offset
                                    self.recalculate_max_scroll();
                                    // Check if we should still be considered scrolled away after resize
                                    if self.scroll_offset < self.max_scroll {
                                        // User might still be scrolled away, keep the flag
                                        // (Unless they were at the bottom before resize)
                                        // This logic could be refined, but for now, let's assume they remain scrolled away if not at the new bottom.
                                    } else {
                                        self.user_scrolled_away = false; // At the new bottom
                                    }
                                }
                                Event::Paste(data) => {
                                    if matches!(self.input_mode, InputMode::Editing) {
                                        let normalized_data = data.replace("\r\n", "\n").replace("\r", "\n");
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
                                            self.perform_scroll(ScrollDirection::Down, ScrollAmount::Line(3))?; // Scroll a few lines per mouse wheel tick
                                            // No need to redraw here, the main loop will handle it
                                        }
                                        MouseEvent {
                                            kind: MouseEventKind::ScrollUp,
                                            ..
                                        } => {
                                           self.perform_scroll(ScrollDirection::Up, ScrollAmount::Line(3))?; // Scroll a few lines per mouse wheel tick
                                            // No need to redraw here, the main loop will handle it
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
        let mut display_items_updated = false;
        match event {
            AppEvent::ThinkingStarted => {
                debug!(target:"tui.handle_app_event", "Setting is_processing = true");
                self.is_processing = true;
                self.spinner_state = 0;
                self.progress_message = None;
            }
            AppEvent::MessagePart { id, delta } => {
                debug!(target:"tui.handle_app_event", "MessagePart: {}", id);
                // Find and update the message with the given ID
                if let Some(item) = self.display_items.iter_mut().find(|di| match di {
                    DisplayItem::Message { id: item_id, .. } => *item_id == id,
                    _ => false,
                }) {
                    // Ensure it's the Message variant before accessing content
                    if let DisplayItem::Message { content, .. } = item {
                        let new_line = Line::from(delta.clone());
                        content.push(new_line);
                        display_items_updated = true;
                    } else {
                        warn!(target:"tui.handle_app_event", "Found DisplayItem for ID {}, but it's not a Message variant.", id);
                    }
                } else {
                    warn!(target:"tui.handle_app_event", "MessagePart received for unknown ID: {}", id);
                }
            }
            AppEvent::ModelChanged { model } => {
                debug!(target:"tui.handle_app_event", "Model changed to: {}", model);
                self.current_model = model;
            }
            AppEvent::ToolCallStarted { name, id, .. } => {
                self.spinner_state = 0;
                self.progress_message = Some(format!("Executing tool: {}", name));
                debug!(target:"tui.handle_app_event", "Tool call started: {} ({:?})", name, id);

                // Find the corresponding raw message
                let raw_msg = self.raw_messages.iter().find(|m| m.id == id).cloned();

                if let Some(raw_msg) = raw_msg {
                    // Add the raw message to the TUI's internal list
                    // self.raw_messages.push(raw_msg.clone()); // Already added by MessageAdded

                    // Format using the actual blocks
                    let formatted = format_message(
                        &raw_msg.content_blocks,
                        Role::Tool,
                        self.terminal.size().map(|r| r.width).unwrap_or(100),
                    );

                    // Check if ID already exists in the TUI's formatted list
                    if self.display_items.iter().any(|di| match di {
                        DisplayItem::Message { id: item_id, .. } => *item_id == id,
                        _ => false,
                    }) {
                        // Tool call might start before MessageAdded event completes streaming,
                        // so just update the progress message if the message already exists.
                        // warn!(target:
                        //     "tui.handle_app_event",
                        //     "ToolCallStarted: Message ID {} already exists. Content blocks: {}",
                        //     id,
                        //     raw_msg.content_blocks.len()
                        // );
                    } else {
                        self.display_items.push(DisplayItem::Message {
                            id: id.clone(),
                            role: Role::Tool,
                            content: formatted,
                        });
                        debug!(target: "tui.handle_app_event", "Added message ID (ToolCallStarted): {}", id);
                        display_items_updated = true;
                    }
                } else {
                    warn!(target: "tui.handle_app_event", "ToolCallStarted event received for ID {}, but corresponding raw message not found yet.", id);
                }
            }
            AppEvent::MessageAdded {
                role,
                content_blocks,
                id,
                ..
            } => {
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
                if self.display_items.iter().any(|di| match di {
                    DisplayItem::Message { id: item_id, .. } => *item_id == id,
                    _ => false,
                }) {
                    info!(
                        target: "tui.handle_app_event",
                        "MessageAdded: ID {} already exists. Content blocks: {}. Skipping.",
                        id,
                        content_blocks.len()
                    );
                } else {
                    let display_item = DisplayItem::Message {
                        id: id.clone(),
                        role,
                        content: formatted,
                    };
                    self.display_items.push(display_item);
                    display_items_updated = true;
                    debug!(
                        target: "tui.handle_app_event",
                        "Added message ID: {} with {} content blocks",
                        id,
                        content_blocks.len()
                    );
                }
            }
            AppEvent::RestoredMessage {
                role,
                content_blocks,
                id,
                model: _,
            } => {
                // Handle restored messages the same as new messages for display purposes
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
                if self.display_items.iter().any(|di| match di {
                    DisplayItem::Message { id: item_id, .. } => *item_id == id,
                    _ => false,
                }) {
                    debug!(
                        target: "tui.handle_app_event",
                        "RestoredMessage: ID {} already exists. Skipping.",
                        id,
                    );
                } else {
                    let display_item = DisplayItem::Message {
                        id: id.clone(),
                        role,
                        content: formatted,
                    };
                    self.display_items.push(display_item);
                    display_items_updated = true;
                    debug!(
                        target: "tui.handle_app_event",
                        "Restored message ID: {} with {} content blocks",
                        id,
                        content_blocks.len()
                    );
                }
            }
            AppEvent::MessageUpdated { id, content } => {
                if self.display_items.iter().any(|di| match di {
                    DisplayItem::Message { id: item_id, .. } => *item_id == id,
                    _ => false,
                }) {
                    debug!(
                        target: "tui.handle_app_event",
                        "Updating message ID: {}", id,
                    );
                    // Now use the blocks from the raw message
                    if let Some(raw_msg) = self.raw_messages.iter().find(|m| m.id == id) {
                        // Find the existing DisplayItem::Message and update its content
                        if let Some(item) = self.display_items.iter_mut().find(|di| match di {
                            DisplayItem::Message { id: item_id, .. } => *item_id == id,
                            _ => false,
                        }) {
                            if let DisplayItem::Message {
                                role: item_role,
                                content: item_content,
                                ..
                            } = item
                            {
                                *item_content = format_message(
                                    &raw_msg.content_blocks,
                                    *item_role, // Use the existing role from the item
                                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                                );
                                display_items_updated = true; // Mark that message content changed
                                debug!(
                                    target: "tui.handle_app_event",
                                    "Updated message ID: {} with new blocks", id,
                                );
                            } else {
                                warn!(target: "tui.handle_app_event", "Found DisplayItem for ID {}, but it's not a Message variant.", id);
                            }
                        } else {
                            warn!(target: "tui.handle_app_event", "MessageUpdated: DisplayItem ID {} not found for raw message.", id);
                            // Optionally create a new DisplayItem if not found
                        }
                    } else {
                        warn!(target: "tui.handle_app_event", "MessageUpdated: Raw message ID {} not found.", id);
                        // Fallback: Update based on string content if raw message is missing
                        if let Some(item) = self.display_items.iter_mut().find(|di| match di {
                            DisplayItem::Message { id: item_id, .. } => *item_id == id,
                            _ => false,
                        }) {
                            if let DisplayItem::Message {
                                role: item_role,
                                content: item_content,
                                ..
                            } = item
                            {
                                let block =
                                    crate::app::conversation::MessageContentBlock::Text(content);
                                *item_content = format_message(
                                    &[block],
                                    *item_role,
                                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                                );
                                display_items_updated = true; // Mark that message content changed
                            } else {
                                warn!(target: "tui.handle_app_event", "Found DisplayItem for ID {} (fallback), but it's not a Message variant.", id);
                            }
                        } else {
                            warn!(target: "tui.handle_app_event", "MessageUpdated: DisplayItem ID {} not found (fallback).", id);
                        }
                    }
                } else {
                    warn!(target: "tui.handle_app_event", "MessageUpdated: ID {} not found.", id);
                }
            }
            AppEvent::ToolCallCompleted {
                name: _,
                result,
                id,
                ..
            } => {
                // If the completed tool matches the *currently* displayed approval,
                // clear the current approval state and activate the next one.
                // This handles cases where a tool might execute automatically (e.g., always approved)
                // while another was pending user input.
                if let Some(current_approval) = &self.current_tool_approval {
                    if current_approval.id == id {
                        // Compare with original tool call ID
                        debug!(target: "tui.handle_app_event", "Tool {} (which was pending approval) completed.", id);
                        self.current_tool_approval = None; // Clear current explicitly
                    // If a tool completes that was the one being approved,
                    // we might want to revert to normal input mode if nothing else is processing.
                    // However, App will send a new RequestToolApproval if there's another,
                    // or ThinkingCompleted if the sequence is done.
                    // For now, just clearing current_tool_approval is safest.
                    // The input mode will be managed by RequestToolApproval or user's response.
                    } else {
                        // A different tool completed, progress message was likely set by ToolCallStarted
                        self.progress_message = None;
                    }
                } else {
                    // No tool was awaiting approval, just clear progress
                    self.progress_message = None;
                }

                debug!(target: "tui.handle_app_event", "Adding Tool Result display item for original call ID: {}", id);

                // Check if result should be truncated
                const MAX_PREVIEW_LINES: usize = 5;
                let lines: Vec<&str> = result.lines().collect();
                let should_truncate = lines.len() > MAX_PREVIEW_LINES;

                let formatted_content = if should_truncate {
                    // Create truncated preview content
                    let preview_content = format!(
                        "{}
... ({} more lines, press 'Ctrl+r' (in normal mode) to toggle full view)",
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
                    format_tool_preview(
                        // Use preview formatter for consistency
                        &result,
                        self.terminal.size().map(|r| r.width).unwrap_or(100),
                    )
                };

                // Generate a unique ID for this display item
                let display_id = format!("result_{}", id);

                let display_item = DisplayItem::ToolResult {
                    id: display_id,
                    tool_use_id: id.clone(), // Store the original tool call ID
                    content: formatted_content,
                    full_result: result.clone(), // Store the full, unformatted result
                    is_truncated: should_truncate,
                };
                self.display_items.push(display_item);
                display_items_updated = true;
            }
            AppEvent::ToolCallFailed {
                name, error, id, ..
            } => {
                self.progress_message = None; // Clear progress on failure

                debug!(target: "tui.handle_app_event", "Adding Tool Failure System Info for ID: {}, Error: {}", id, error);

                // Create a NEW SystemInfo item to display the tool failure
                let failure_content = format!("Tool '{}' failed: {}. (ID: {})", name, error, id);
                let formatted_failure_lines = vec![Line::from(Span::styled(
                    failure_content,
                    Style::default().fg(Color::Red),
                ))];

                // Generate a unique ID for this display item
                let display_id = format!("failed_{}", id);

                let display_item = DisplayItem::SystemInfo {
                    id: display_id,
                    content: formatted_failure_lines,
                };
                self.display_items.push(display_item);
                display_items_updated = true;
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
                self.current_tool_approval = Some(approval_info);
                self.input_mode = InputMode::AwaitingApproval;
                debug!(target: "tui.handle_app_event", "Set current tool approval for {} ({}). Mode: AwaitingApproval.", name, id);
            }
            AppEvent::CommandResponse { content, id: _ } => {
                let response_id = format!("cmd_resp_{}", chrono::Utc::now().timestamp_millis());
                // Use the new formatter
                let formatted = format_command_response(
                    &content,
                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                );
                // Add the raw message (optional, maybe remove raw_messages later?)
                // self.raw_messages
                //     .push(crate::app::Message::new_text(Role::System, content.clone())); // Keep for raw log
                // Create CommandResponse display item
                self.display_items.push(DisplayItem::CommandResponse {
                    id: response_id,
                    content: formatted,
                });
                display_items_updated = true;
            }
            AppEvent::OperationCancelled { info } => {
                self.is_processing = false;
                self.progress_message = None;
                self.current_tool_approval = None;
                self.input_mode = InputMode::Normal;
                self.spinner_state = 0;
                let cancellation_text = format!("Operation cancelled: {}", info);
                let display_id = format!("cancellation_{}", chrono::Utc::now().timestamp_millis());
                self.display_items.push(DisplayItem::SystemInfo {
                    id: display_id,
                    content: format_command_response(
                        &cancellation_text,
                        self.terminal.size().map(|r| r.width).unwrap_or(100),
                    ),
                });
                display_items_updated = true;
            }
            AppEvent::ThinkingCompleted | AppEvent::Error { .. } => {
                debug!(target:"tui.handle_app_event", "Setting is_processing = false (ThinkingCompleted/Error)");
                self.is_processing = false;
                self.progress_message = None;
            }
        }

        // --- Handle Scrolling After Display Items Updates ---
        if display_items_updated {
            // Recalculate max scroll based on the *potentially updated* display items list
            self.recalculate_max_scroll(); // This needs to be called first

            // --- Auto-scroll logic ---
            if !self.user_scrolled_away {
                let new_max_scroll = self.get_max_scroll(); // Get the updated max scroll
                self.set_scroll_offset(new_max_scroll);
                debug!(target: "tui.handle_app_event", "Auto-scrolled to bottom (offset={})", new_max_scroll);
            } else {
                debug!(target: "tui.handle_app_event", "Display updated, but user scrolled away (offset={}, max_scroll={}). Skipping auto-scroll.", self.scroll_offset, self.max_scroll);
            }
            // --- End Auto-scroll logic ---
        }
    }

    fn get_scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn set_scroll_offset(&mut self, offset: usize) {
        let clamped_offset = offset.min(self.max_scroll);
        // Only update if the offset actually changes
        if clamped_offset != self.scroll_offset {
            self.scroll_offset = clamped_offset;
            debug!(target: "tui.set_scroll_offset", "Set scroll_offset={}, max_scroll={}", self.scroll_offset, self.max_scroll);
            // Update user_scrolled_away based on the *new* offset
            self.user_scrolled_away = self.scroll_offset < self.max_scroll;
            debug!(target: "tui.set_scroll_offset", "user_scrolled_away={}", self.user_scrolled_away);
        }
    }

    fn get_max_scroll(&self) -> usize {
        self.max_scroll
    }

    fn set_max_scroll(&mut self, max: usize) {
        self.max_scroll = max;
        // Clamp current offset if it's now beyond the new max
        if self.scroll_offset > self.max_scroll {
            let old_offset = self.scroll_offset;
            self.scroll_offset = self.max_scroll;
            debug!(target: "tui.set_max_scroll", "Clamped scroll_offset from {} to {} due to new max_scroll={}", old_offset, self.scroll_offset, self.max_scroll);
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
        current_tool_approval_info: Option<&PendingApprovalInfo>,
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
        display_items: &[DisplayItem],
        input_mode: InputMode,
        scroll_offset: usize,
        max_scroll: usize,
        current_model: &str,
        current_approval_info: Option<&PendingApprovalInfo>,
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
        Self::render_display_items_static(
            f,
            messages_area,
            display_items,
            scroll_offset,
            max_scroll,
        );

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

    fn render_display_items_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        display_items: &[DisplayItem],
        scroll_offset: usize,
        max_scroll: usize,
    ) {
        if display_items.is_empty() {
            let placeholder = Paragraph::new("No messages yet...")
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false });
            f.render_widget(placeholder, area);
            return;
        }

        // Flatten display item content lines for calculating total height and rendering
        let all_lines: Vec<Line> = display_items
            .iter()
            .flat_map(|di| match di {
                // Extract content based on the variant using named fields
                DisplayItem::Message { content, .. }
                | DisplayItem::ToolResult { content, .. }
                | DisplayItem::CommandResponse { content, .. }
                | DisplayItem::SystemInfo { content, .. } => content.clone(),
            })
            .collect();
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

        // --- Global keys handled first ---

        // Handle ESC specifically for denying the current tool approval
        if key.code == KeyCode::Esc && self.input_mode == InputMode::AwaitingApproval {
            if let Some(approval_info) = self.current_tool_approval.take() {
                action = Some(InputAction::DenyTool(approval_info.id));
                self.input_mode = InputMode::Normal; // Revert to normal mode
                return Ok(action);
            }
        }

        // --- Scrolling and other non-mode-specific keys ---
        // Check if we are *not* in editing mode for scroll keys etc.
        if self.input_mode != InputMode::Editing {
            match key.code {
                // Full Page Scroll Up
                KeyCode::PageUp => {
                    self.perform_scroll(ScrollDirection::Up, ScrollAmount::FullPage)?;
                    return Ok(None); // Just scroll
                }
                // Full Page Scroll Down
                KeyCode::PageDown => {
                    self.perform_scroll(ScrollDirection::Down, ScrollAmount::FullPage)?;
                    return Ok(None); // Just scroll
                }

                // Line Scroll Up (Arrows)
                KeyCode::Up => {
                    self.perform_scroll(ScrollDirection::Up, ScrollAmount::Line(1))?;
                    return Ok(None); // Just scroll
                }
                // Line Scroll Down (Arrows)
                KeyCode::Down => {
                    self.perform_scroll(ScrollDirection::Down, ScrollAmount::Line(1))?;
                    return Ok(None); // Just scroll
                }

                // Line Scroll Up (k in Normal Mode only)
                KeyCode::Char('k') if self.input_mode == InputMode::Normal => {
                    self.perform_scroll(ScrollDirection::Up, ScrollAmount::Line(1))?;
                    return Ok(None); // Just scroll
                }
                // Line Scroll Down (j in Normal Mode only)
                KeyCode::Char('j') if self.input_mode == InputMode::Normal => {
                    self.perform_scroll(ScrollDirection::Down, ScrollAmount::Line(1))?;
                    return Ok(None); // Just scroll
                }

                // Half Page Scroll Up (u/Ctrl+u in Normal Mode only)
                KeyCode::Char('u') if self.input_mode == InputMode::Normal => {
                    if key.modifiers == KeyModifiers::CONTROL || key.modifiers == KeyModifiers::NONE
                    {
                        self.perform_scroll(ScrollDirection::Up, ScrollAmount::HalfPage)?;
                        return Ok(None); // Just scroll
                    }
                }
                // Half Page Scroll Down (d in Normal Mode only)
                KeyCode::Char('d')
                    if (self.input_mode == InputMode::Normal
                        && key.modifiers == KeyModifiers::NONE) =>
                {
                    self.perform_scroll(ScrollDirection::Down, ScrollAmount::HalfPage)?;
                    return Ok(None); // Just scroll
                }

                // Toggle Tool Result Truncation (Ctrl+R)
                KeyCode::Char('r') | KeyCode::Char('R')
                    if key.modifiers == KeyModifiers::CONTROL =>
                {
                    let scroll_offset = self.get_scroll_offset();
                    // Find the message corresponding to the current view port top
                    let mut current_line_index = 0;
                    let mut target_display_item_id = None;

                    for item in &self.display_items {
                        let item_line_count = match item {
                            DisplayItem::Message { content, .. } => content.len(),
                            DisplayItem::ToolResult { content, .. } => content.len(),
                            DisplayItem::CommandResponse { content, .. } => content.len(),
                            DisplayItem::SystemInfo { content, .. } => content.len(),
                        };

                        if scroll_offset >= current_line_index
                            && scroll_offset < current_line_index + item_line_count
                        {
                            // We found the item at the top of the viewport
                            if let DisplayItem::ToolResult { id, .. } = item {
                                target_display_item_id = Some(id.clone());
                            }
                            break;
                        }
                        current_line_index += item_line_count;
                    }

                    if let Some(target_id) = target_display_item_id {
                        action = Some(InputAction::ToggleMessageTruncation(target_id));
                    } else {
                        debug!(target: "tui.handle_input", "Ctrl+R: No tool result found at scroll offset {}", scroll_offset);
                    }
                    return Ok(action);
                }

                // Add a wildcard arm to make the match exhaustive for non-editing mode keys handled here
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
                    // Send Message
                    KeyCode::Enter => {
                        let input = self.textarea.lines().join("\n");
                        if input.trim().is_empty() {
                            return Ok(action);
                        }
                        self.clear_textarea();
                        action = Some(InputAction::SendMessage(input));
                        return Ok(action);
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
                    _ => {} // Ignore other keys in Normal mode (covered by global/scroll)
                }
            }
            InputMode::Editing => {
                match key.code {
                    // Stop editing
                    KeyCode::Esc => {
                        self.input_mode = InputMode::Normal;
                    }
                    // Send Message
                    KeyCode::Enter => {
                        let input = self.textarea.lines().join("\n");
                        if !input.trim().is_empty() {
                            action = Some(InputAction::SendMessage(input));
                            // Re-initialize the textarea to clear it
                            let mut new_textarea = TextArea::default();
                            new_textarea
                                .set_block(self.textarea.block().cloned().unwrap_or_default());
                            new_textarea.set_placeholder_text(self.textarea.placeholder_text());
                            new_textarea.set_style(self.textarea.style());
                            self.textarea = new_textarea;
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
                self.command_tx
                    .send(AppCommand::ProcessUserInput(input))
                    .await?;
                // Clear scroll lock when user sends a message
                self.user_scrolled_away = false;
                debug!(target: "tui.dispatch_input_action", "Sent UserInput command, user_scrolled_away=false");
            }
            InputAction::ToggleMessageTruncation(target_id) => {
                let mut found_and_toggled = false;
                let mut scroll_adjustment = 0; // Track line count difference

                if let Some(item) = self.display_items.iter_mut().find(|di| match di {
                    DisplayItem::ToolResult { id, .. } => *id == target_id,
                    _ => false,
                }) {
                    if let DisplayItem::ToolResult {
                        content,
                        full_result,
                        is_truncated,
                        .. // Ignore tool_use_id
                    } = item
                    {
                        let old_line_count = content.len();
                        *is_truncated = !*is_truncated;

                        let new_content = if *is_truncated {
                            // Re-truncate
                            const MAX_PREVIEW_LINES: usize = 5;
                            let lines: Vec<&str> = full_result.lines().collect();
                            let preview_content = format!(
                                "{}
... ({} more lines, press 'Ctrl+r' (in normal mode) to toggle full view)",
                                lines
                                    .iter()
                                    .take(MAX_PREVIEW_LINES)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                lines.len().saturating_sub(MAX_PREVIEW_LINES)
                            );
                            format_tool_preview(
                                &preview_content,
                                self.terminal.size().map(|r| r.width).unwrap_or(100),
                            )
                        } else {
                            // Expand to full result
                            format_tool_preview(
                                full_result,
                                self.terminal.size().map(|r| r.width).unwrap_or(100),
                            )
                        };

                        *content = new_content;
                        found_and_toggled = true;
                        let new_line_count = content.len();
                        scroll_adjustment = new_line_count as isize - old_line_count as isize;
                        debug!(target: "tui.dispatch_input_action", "Toggled truncation for ToolResult {}, is_truncated={}, line diff={}", target_id, *is_truncated, scroll_adjustment);
                    }
                }

                if found_and_toggled {
                    // Recalculate max scroll due to content change
                    self.recalculate_max_scroll();
                    // Attempt to adjust scroll offset to keep the toggled item roughly in view
                    // This is imperfect but better than jumping abruptly.
                    let current_offset = self.get_scroll_offset();
                    if scroll_adjustment > 0 {
                        // Content grew, increase offset by the difference
                        self.set_scroll_offset(current_offset + scroll_adjustment as usize);
                    } else {
                        // Content shrank, decrease offset, ensuring it doesn't go below zero
                        self.set_scroll_offset(
                            current_offset.saturating_sub((-scroll_adjustment) as usize),
                        );
                    }
                    debug!(target: "tui.dispatch_input_action", "Adjusted scroll offset after toggle: {}", self.scroll_offset);
                }
            }
            InputAction::ApproveToolNormal(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: false,
                    })
                    .await?;
                // Let activate_next_approval handle the current_tool_approval state
                self.activate_next_approval(); // Activate next
            }
            InputAction::ApproveToolAlways(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: true,
                    })
                    .await?;
                // Let activate_next_approval handle the current_tool_approval state
                self.activate_next_approval(); // Activate next
            }
            InputAction::DenyTool(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: false,
                        always: false,
                    })
                    .await?;
                // Let activate_next_approval handle the current_tool_approval state
                self.activate_next_approval(); // Activate next
            }
            InputAction::CancelProcessing => {
                self.command_tx.send(AppCommand::CancelProcessing).await?;
            }
            InputAction::Exit => {
                should_exit = true;
            }
        }
        Ok(should_exit)
    }

    // New function to activate the next pending approval
    fn activate_next_approval(&mut self) {
        if let Some(next_approval) = self.current_tool_approval.take() {
            debug!(target: "tui.activate_next_approval", "Activating next tool approval: {} ({})", next_approval.name, next_approval.id);
            self.current_tool_approval = Some(next_approval);
            self.input_mode = InputMode::AwaitingApproval;
        } else {
            debug!(target: "tui.activate_next_approval", "No more pending approvals. Returning to Normal mode.");
            self.current_tool_approval = None;
            // Always return to Normal mode when the queue is empty after an approval action.
            // The is_processing flag will handle the spinner display separately.
            self.input_mode = InputMode::Normal;
        }
    }

    // Recalculates max scroll based on current display items and terminal size
    fn recalculate_max_scroll(&mut self) {
        let all_lines: Vec<Line> = self
            .display_items
            .iter()
            .flat_map(|di| match di {
                DisplayItem::Message { content, .. } => content.clone(),
                DisplayItem::ToolResult { content, .. } => content.clone(),
                DisplayItem::CommandResponse { content, .. } => content.clone(),
                DisplayItem::SystemInfo { content, .. } => content.clone(),
            })
            .collect();
        let total_lines_count = all_lines.len();

        if let Ok(term_size) = self.terminal.size() {
            let input_height = (self.textarea.lines().len() as u16 + 2)
                .min(MAX_INPUT_HEIGHT)
                .min(term_size.height);
            let messages_area_height = term_size
                .height
                .saturating_sub(input_height)
                .saturating_sub(2); // Account for borders

            let new_max_scroll = if total_lines_count > messages_area_height as usize {
                total_lines_count.saturating_sub(messages_area_height as usize)
            } else {
                0
            };
            self.set_max_scroll(new_max_scroll);
            debug!(target: "tui.recalculate_max_scroll", "Recalculated max_scroll={}, total_lines={}, area_height={}", new_max_scroll, total_lines_count, messages_area_height);
        } else {
            warn!(target: "tui.recalculate_max_scroll", "Failed to get terminal size for scroll update.");
        }
    }

    // --- Scrolling Helpers ---

    fn get_page_height(&self) -> Result<usize> {
        let term_size = self.terminal.size()?;
        let input_height = (self.textarea.lines().len() as u16 + 2)
            .min(MAX_INPUT_HEIGHT)
            .min(term_size.height);
        let messages_area_height = term_size
            .height
            .saturating_sub(input_height)
            .saturating_sub(2); // Account for borders
        Ok(messages_area_height as usize)
    }

    fn perform_scroll(&mut self, direction: ScrollDirection, amount: ScrollAmount) -> Result<()> {
        let current_offset = self.get_scroll_offset();
        let max_scroll = self.get_max_scroll();
        let page_height = self.get_page_height()?;

        let scroll_lines = match amount {
            ScrollAmount::Line(lines) => lines,
            ScrollAmount::HalfPage => page_height / 2,
            ScrollAmount::FullPage => page_height,
        };

        let new_offset = match direction {
            ScrollDirection::Up => current_offset.saturating_sub(scroll_lines),
            ScrollDirection::Down => (current_offset + scroll_lines).min(max_scroll),
        };

        // Update offset using the setter which handles clamping and user_scrolled_away
        self.set_scroll_offset(new_offset);

        Ok(())
    }

    // --- End Scrolling Helpers ---

    // New function to clear the textarea and reset cursor
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
