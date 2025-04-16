use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use std::io::{self, Stdout};
use std::time::{Duration, Instant};
use tui_textarea::{Input, Key, TextArea};

use tokio::select;

use crate::app::command::AppCommand;
use crate::app::{AppEvent, Role};
use crate::utils::logging::{debug, error, info, warn};
use tokio::{
    sync::mpsc::{self},
    task::JoinHandle,
};

mod message_formatter;

use message_formatter::{format_message, format_tool_preview, format_tool_result_block};

const MAX_INPUT_HEIGHT: u16 = 10;
const SPINNER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

// UI States
#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Normal,
    Editing,
    AwaitingApproval,
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    textarea: TextArea<'static>,
    input_mode: InputMode,
    messages: Vec<FormattedMessage>, // No Arc<Mutex<>>
    is_processing: bool,             // No Arc<RwLock<>>
    // Progress tracking
    progress_message: Option<String>, // No Arc<Mutex<>>
    last_spinner_update: Instant,
    spinner_state: usize, // Spinner state is now instance state
    // Tool approval related state
    command_tx: mpsc::Sender<AppCommand>, // Store directly, required
    approval_request: Option<(String, String, serde_json::Value)>, // No Arc<Mutex<>>
    // Scroll state
    scroll_offset: usize,
    max_scroll: usize,
    user_scrolled_away: bool, // Track if user manually scrolled away from bottom
    // Store messages with their original block structure for potential later use
    // (e.g., toggling raw tool result view if needed)
    raw_messages: Vec<crate::app::Message>,
}

#[derive(Clone)]
pub struct FormattedMessage {
    content: Vec<Line<'static>>,
    role: Role,
    id: String,
    full_tool_result: Option<String>,
    is_truncated: bool,
    tool_name: Option<String>,
}

// Define possible actions that might need async processing (for sending commands)
#[derive(Debug)]
enum InputAction {
    SendMessage(String),
    ToggleMessageTruncation(String),
    ApproveToolNormal(String),
    ApproveToolAlways(String),
    DenyTool(String),
    Exit,
}

impl Tui {
    pub fn new(command_tx: mpsc::Sender<AppCommand>) -> Result<Self> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
            EnableBracketedPaste
        )?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        // Reset scroll values - Removed static access
        // if let Ok(mut offset) = SCROLL_OFFSET.write() {
        //     *offset = 0;
        // }
        // if let Ok(mut max) = MAX_SCROLL.write() {
        //     *max = 0;
        // }

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
            messages: Vec::new(),   // Initialize directly
            is_processing: false,   // Initialize directly
            progress_message: None, // Initialize directly
            last_spinner_update: Instant::now(),
            spinner_state: 0,          // Initialize spinner state
            command_tx,                // Store the sender
            approval_request: None,    // Initialize directly
            scroll_offset: 0,          // Initialize scroll offset
            max_scroll: 0,             // Initialize max scroll
            user_scrolled_away: false, // Initialize user_scrolled_away flag
            raw_messages: Vec::new(),  // Initialize raw_messages
        })
    }

    // Cleanup function to restore terminal
    fn cleanup_terminal(&mut self) -> Result<()> {
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags
        )?;
        disable_raw_mode()?;
        Ok(())
    }

    pub async fn run(&mut self, mut event_rx: mpsc::Receiver<AppEvent>) -> Result<()> {
        // Spawn a task to read terminal events because crossterm::event::read is blocking
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
                // Add a small sleep to prevent tight looping when no events occur
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        let mut should_exit = false;
        while !should_exit {
            // Update state needed before drawing (like spinner)
            self.update_spinner_state();

            // --- Prepare State for Drawing ---
            let input_mode = self.input_mode; // Copy
            let is_processing = self.is_processing; // Copy
            // Clone progress message to detach lifetime from self
            let progress_message_owned: Option<String> = self.progress_message.clone();
            // Get owned spinner char to detach lifetime from self
            let spinner_char_owned: String = self.get_spinner_char(); // Now returns String

            // Create input block using owned/copied data
            let input_block = Tui::create_input_block_static(
                // static call
                input_mode,
                is_processing,
                progress_message_owned, // Pass owned Option<String>
                spinner_char_owned,     // Pass owned String
            );
            // Apply the block to the actual textarea BEFORE the draw call
            self.textarea.set_block(input_block); // &mut self.textarea

            // --- Draw UI ---
            // Only need immutable references inside the closure, created locally
            self.terminal.draw(|f| {
                // &mut self.terminal
                // Get local immutable refs inside closure
                let textarea_ref = &self.textarea; // &self.textarea
                let messages_ref = &self.messages; // &self.messages

                // Render UI using static method with local immutable refs
                if let Err(e) = Tui::render_ui_static(
                    f,
                    textarea_ref,
                    messages_ref,
                    input_mode, // Use the copied input_mode
                    self.scroll_offset,
                    self.max_scroll,
                ) {
                    error("tui.run.draw", &format!("UI rendering failed: {}", e));
                    // Log error within closure
                }
            })?;

            // Event Handling with select!
            select! {
                // Bias select to prioritize terminal input slightly if needed, but usually not necessary
                // biased;

                // Handle terminal input event
                maybe_term_event = term_event_rx.recv() => {
                    match maybe_term_event {
                        Some(Ok(event)) => {
                            // Handle resize/paste directly
                            match event {
                                Event::Resize(_, _) => {
                                    debug("tui.run", "Terminal resized");
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
                                    self.set_max_scroll(new_max_scroll); // This clamps scroll_offset if needed

                                    // Check if user should now be considered "at the bottom" after resize
                                    if self.scroll_offset == self.max_scroll {
                                        self.user_scrolled_away = false;
                                    }
                                }
                                Event::Paste(data) => {
                                    if matches!(self.input_mode, InputMode::Editing) {
                                        let normalized_data = data.replace("\\r\\n", "\\n").replace("\\r", "\\n");
                                        self.textarea.insert_str(&normalized_data);
                                        debug("tui.run", &format!("Pasted {} chars", normalized_data.len()));
                                    }
                                }
                                Event::Key(key) => {
                                    // Process key event using handle_input
                                    match self.handle_input(key).await {
                                        Ok(Some(action)) => {
                                            debug("tui.run", &format!("Handling input action: {:?}", action));
                                             // Dispatch action (potentially sending commands)
                                            if self.dispatch_input_action(action).await? {
                                                 should_exit = true;
                                            }
                                        }
                                        Ok(None) => {} // No action needed
                                        Err(e) => {
                                            error("tui.run", &format!("Error handling input: {}", e));
                                        }
                                    }
                                }
                                Event::FocusGained => debug("tui.run", "Focus gained"),
                                Event::FocusLost => debug("tui.run", "Focus lost"),
                                Event::Mouse(_) => {} // Ignore mouse
                            }
                        }
                        Some(Err(e)) => {
                            error("tui.run", &format!("Error reading terminal event: {}", e));
                            // Decide if we should exit on error
                            // should_exit = true;
                        }
                        None => {
                            // Channel closed, input task likely ended
                            info("tui.run", "Terminal event channel closed.");
                            should_exit = true;
                        }
                    }
                }

                // Handle application event
                maybe_app_event = event_rx.recv() => {
                    match maybe_app_event {
                        Some(event) => {
                            // No longer calculate max scroll here - now done after
                            // message updates within handle_app_event

                            // Process the AppEvent directly using self
                            self.handle_app_event(event).await;
                        }
                        None => {
                            // App closed the channel, maybe exit?
                            info("tui.run", "App event channel closed.");
                            should_exit = true; // Exit if the App task ends
                        }
                    }
                }

                // Add a small timeout to ensure the loop continues even without events
                // This allows the spinner to update visually.
                _ = tokio::time::sleep(SPINNER_UPDATE_INTERVAL / 2) => {}
            }
        }

        // Cleanup terminal before exiting run loop
        self.cleanup_terminal()?;
        Ok(())
    }

    // Removed spawn_event_handler - logic moved into run loop

    // Handle AppEvents directly within Tui
    async fn handle_app_event(&mut self, event: AppEvent) {
        // Flag to indicate if messages were added and scroll needs adjustment
        let mut messages_updated = false;

        // Update is_processing state based on events
        match event {
            AppEvent::ThinkingStarted => {
                debug("tui.handle_app_event", "Setting is_processing = true");
                self.is_processing = true;
                self.spinner_state = 0; // Reset spinner
                self.progress_message = None; // Clear specific progress message initially
            }
            AppEvent::ThinkingCompleted | AppEvent::Error { .. } => {
                debug("tui.handle_app_event", "Setting is_processing = false");
                self.is_processing = false;
                self.progress_message = None; // Clear progress message
            }
            AppEvent::ToolCallStarted { name, id } => {
                self.spinner_state = 0; // Reset spinner for tool call visual
                // Optionally update progress message
                self.progress_message = Some(format!("Executing tool: {}", name));
                // Add a placeholder message for the tool call if needed for UI sync,
                // Or rely on the assistant message that contains the ToolCall block.
                // Current approach: The assistant message containing ToolCall is added via MessageAdded.
                debug(
                    "tui.handle_app_event",
                    &format!("Tool call started: {} ({:?})", name, id),
                );

                // --- IDEAL: Use content_blocks from AppEvent --- //
                // Find the corresponding raw message (should have been added just before this event)
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
                        warn(
                            "tui.handle_app_event",
                            &format!("MessageAdded: ID {} already exists. Skipping.", id),
                        );
                    } else {
                        let formatted_message = FormattedMessage {
                            content: formatted,
                            role: Role::Tool,
                            id: id.clone(),
                            full_tool_result: None,
                            is_truncated: false,
                            tool_name: None,
                        };
                        self.messages.push(formatted_message);
                        // self.raw_messages.push(crate::app::Message::new_text(role, content.clone())); // Raw message added above
                        debug("tui.handle_app_event", &format!("Added message ID: {}", id));
                        messages_updated = true; // Mark that messages changed
                    }
                } else {
                    // This case might happen if the App adds a message but fails to send the event,
                    // or if the event arrives before the App could add it (less likely).
                    warn(
                        "tui.handle_app_event",
                        &format!(
                            "MessageAdded event received for ID {}, but corresponding raw message not found yet.",
                            id
                        ),
                    );
                    // Optionally, add a placeholder formatted message
                    // let placeholder_block = crate::app::conversation::MessageContentBlock::Text("[Message content pending...]".to_string());
                    // let formatted = format_message(&[placeholder_block], role);
                    // ... add formatted message ...
                }
            }
            AppEvent::ToolBatchProgress { batch_id } => {
                // The TUI no longer receives current/total/name here.
                // We can log the batch ID or set a generic progress message if needed.
                debug(
                    "tui.handle_app_event",
                    &format!("Processing batch {}", batch_id),
                );
                self.progress_message = Some(format!("Processing tool batch {}", batch_id)); // Example message
                self.is_processing = true; // Ensure processing state is active during batch
            }
            AppEvent::MessageAdded {
                role,
                content_blocks,
                id,
            } => {
                if role == Role::System {
                    debug("tui.handle_app_event", "Skipping system message display");
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
                    info(
                        "tui.handle_app_event",
                        &format!(
                            "MessageAdded: ID {} already exists. Content blocks: {}. Skipping.",
                            id,
                            content_blocks.len()
                        ),
                    );
                } else {
                    let formatted_message = FormattedMessage {
                        content: formatted,
                        role,
                        id: id.clone(),         // Use the ID from the event
                        full_tool_result: None, // Initialize appropriately
                        is_truncated: false,    // Initialize appropriately
                        // Determine tool_name if it's a ToolCall block
                        tool_name: content_blocks
                            .iter()
                            .find_map(|block| {
                                if let crate::app::conversation::MessageContentBlock::ToolCall(tc) = block {
                                    Some(tc.name.clone())
                                } else if let crate::app::conversation::MessageContentBlock::ToolResult { .. } = block {
                                    Some("Tool Result".to_string()) // Or extract from ID?
                                } else {
                                    None
                                }
                            })
                            .or_else(|| {
                                if role == Role::Tool {
                                    Some("Tool Result".to_string())
                                } else {
                                    None
                                }
                            }),
                    };
                    self.messages.push(formatted_message);
                    messages_updated = true; // Mark that messages changed
                    debug(
                        "tui.handle_app_event",
                        &format!(
                            "Added message ID: {} with {} content blocks",
                            id,
                            content_blocks.len()
                        ),
                    );
                }
            }
            AppEvent::MessageUpdated { id, content } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                    debug(
                        "tui.handle_app_event",
                        &format!("Updating message ID: {}", id),
                    );
                    // Now use the blocks from the raw message
                    if let Some(raw_msg) = self.raw_messages.iter().find(|m| m.id == id) {
                        msg.content = format_message(
                            &raw_msg.content_blocks,
                            msg.role,
                            self.terminal.size().map(|r| r.width).unwrap_or(100),
                        );
                        messages_updated = true; // Mark that message content changed
                        debug(
                            "tui.handle_app_event",
                            &format!("Updated message ID: {} with new blocks", id),
                        );
                    } else {
                        warn(
                            "tui.handle_app_event",
                            &format!("MessageUpdated: Raw message ID {} not found.", id),
                        );
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
                    warn(
                        "tui.handle_app_event",
                        &format!("MessageUpdated: ID {} not found.", id),
                    );
                }
            }
            AppEvent::ToolCallCompleted {
                name: _, // Name is implicitly shown in the formatted result block
                result,
                id, // Use this ID for the formatted message
            } => {
                self.progress_message = None; // Clear progress on completion

                // Create a NEW message to display the tool result
                debug(
                    "tui.handle_app_event",
                    &format!("Adding Tool Result message for ID: {}", id),
                );

                // Use the tool result formatter directly
                let formatted_result_lines = format_tool_result_block(
                    &id,
                    &result,
                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                );

                let formatted_message = FormattedMessage {
                    content: formatted_result_lines,
                    role: Role::Tool,
                    id: format!("result_{}", id), // Ensure unique ID for the result display
                    full_tool_result: Some(result), // Store the full result for toggling
                    is_truncated: false, // Start untruncated (or apply truncation logic here)
                    tool_name: None,     // Tool name is part of the formatted block
                };
                self.messages.push(formatted_message);
                messages_updated = true; // Mark that messages changed
            }
            AppEvent::ToolCallFailed { name, error, id } => {
                self.progress_message = None; // Clear progress on failure

                // Similar logic to ToolCallCompleted: Assuming this relates to a Role::Tool message
                // added via MessageAdded.
                debug(
                    "tui.handle_app_event",
                    &format!(
                        "Adding Tool Failure message for ID: {}, Error: {}",
                        id, error
                    ),
                );

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
                    tool_name: Some(name),
                };
                self.messages.push(formatted_message);
                messages_updated = true; // Mark that messages changed
            }
            AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
            } => {
                // Store approval request state directly on self
                self.approval_request = Some((id.clone(), name.clone(), parameters));
                self.progress_message = Some(format!("Waiting for tool approval: {}", name));
                self.is_processing = true; // Keep spinner active during approval wait

                // Set input mode to AwaitingApproval
                self.input_mode = InputMode::AwaitingApproval;

                // Add a message indicating approval needed
                let approval_text = format!(
                    "Awaiting approval for tool: {} (y/n, Shift+Tab for always)",
                    name
                );
                let block = crate::app::conversation::MessageContentBlock::Text(approval_text);
                let formatted = format_message(
                    &[block],
                    Role::Tool,
                    self.terminal.size().map(|r| r.width).unwrap_or(100),
                );
                // Add a placeholder to raw_messages for consistency, though it won't have real blocks
                self.raw_messages.push(crate::app::Message::new_text(
                    Role::Tool,
                    format!("Approval Prompt for {}", name),
                ));
                self.messages.push(FormattedMessage {
                    content: formatted,
                    role: Role::Tool,
                    id: format!("approval_{}", id), // Unique ID for this prompt
                    full_tool_result: None,
                    is_truncated: false,
                    tool_name: Some(name.clone()),
                });
                messages_updated = true; // Mark that messages changed
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
                    .push(crate::app::Message::new_text(Role::System, content)); // Use actual content for raw
                self.messages.push(FormattedMessage {
                    content: formatted,
                    role: Role::System,
                    id: response_id,
                    full_tool_result: None,
                    is_truncated: false,
                    tool_name: None,
                });
                messages_updated = true; // Mark that messages changed
            }

            AppEvent::ToggleMessageTruncation { id } => {
                // Handle toggling directly in TUI state
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == id) {
                    if msg.role == Role::Tool {
                        if let Some(full_result) = &msg.full_tool_result {
                            msg.is_truncated = !msg.is_truncated;
                            if msg.is_truncated {
                                const MAX_PREVIEW_LINES: usize = 5;
                                let lines: Vec<&str> = full_result.lines().collect();
                                let preview_content = if lines.len() > MAX_PREVIEW_LINES {
                                    format!(
                                        "{}\n... ({} more lines, press 't' to toggle full view)",
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
                            messages_updated = true; // Mark that message content changed
                        }
                    }
                }
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
                self.set_max_scroll(new_max_scroll); // Update max_scroll (clamps current offset if needed)

                // Scroll to bottom ONLY if user wasn't scrolled away
                if !self.user_scrolled_away {
                    debug(
                        "tui.handle_app_event",
                        "Scrolling to bottom after message update.",
                    );
                    self.set_scroll_offset(self.max_scroll); // Use the newly calculated max_scroll
                } else {
                    debug(
                        "tui.handle_app_event",
                        "User scrolled away, not scrolling to bottom.",
                    );
                    // If the update caused the current offset to become the max, reset the flag
                    if self.scroll_offset == self.max_scroll {
                        self.user_scrolled_away = false;
                    }
                }
            } else {
                warn(
                    "tui.handle_app_event",
                    "Failed to get terminal size for scroll update.",
                );
            }
        }
    }

    // Instance-based scroll methods
    fn get_scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn set_scroll_offset(&mut self, offset: usize) {
        // Clamp offset against max_scroll
        let clamped_offset = offset.min(self.max_scroll);
        self.scroll_offset = clamped_offset;
    }

    fn get_max_scroll(&self) -> usize {
        self.max_scroll
    }

    fn set_max_scroll(&mut self, max: usize) {
        self.max_scroll = max;
        // Adjust current scroll offset if it's now out of bounds
        if self.scroll_offset > self.max_scroll {
            self.scroll_offset = self.max_scroll;
        }
    }

    // Spinner methods using instance state
    // Return owned String to avoid lifetime issues with self
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

    // Static method to create input block based on state passed in
    // Takes owned Strings now
    fn create_input_block_static<'a>(
        input_mode: InputMode,
        is_processing: bool,
        progress_message: Option<String>, // Owned Option<String>
        spinner_char: String,             // Owned String
    ) -> Block<'a> {
        let input_border_style = match input_mode {
            InputMode::Editing => Style::default().fg(Color::Yellow),
            InputMode::Normal => Style::default().fg(Color::DarkGray),
            InputMode::AwaitingApproval => {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            }
        };
        let input_title: &str = match input_mode {
            InputMode::Editing => "Input (Esc to stop editing, Enter to send)",
            InputMode::Normal => "Input (i or Ctrl+S to edit, Enter to send, q to quit)",
            InputMode::AwaitingApproval => "Approval Required (y/n, Shift+Tab=always, Esc=deny)",
        };
        let mut input_block = Block::<'a>::default()
            .borders(Borders::ALL)
            .title(input_title)
            .style(input_border_style);

        if is_processing {
            let progress_msg = progress_message.as_deref().unwrap_or_default(); // Get &str from owned Option<String>
            // Add type annotation for title (using ratatui::text::Text)
            let title: ratatui::text::Text =
                format!(" {} Processing {} ", &spinner_char, progress_msg).into(); // Use &spinner_char
            // Use a more specific title if awaiting approval
            // Add type annotation for final_title
            let final_title: ratatui::text::Text = if input_mode == InputMode::AwaitingApproval {
                format!(
                    " {} {} ",
                    &spinner_char, // Use &spinner_char
                    progress_message.as_deref().unwrap_or("Awaiting Approval")  // Get &str
                )
                .into()
            } else {
                title
            };
            // Convert Text to Line for Block::title
            input_block = input_block.title(
                // Get the first line from the Text object
                final_title
                    .lines
                    .get(0)
                    .cloned()
                    .unwrap_or_default()
                    .style(final_title.style) // Apply style from Text
                    .white(),
            );
        }
        input_block
    }

    // Render UI - Static method
    fn render_ui_static(
        f: &mut ratatui::Frame<'_>,
        textarea: &TextArea<'_>,       // Pass textarea immutably
        messages: &[FormattedMessage], // Pass messages slice
        input_mode: InputMode,         // Pass input mode copy
        scroll_offset: usize,          // Pass scroll state
        max_scroll: usize,             // Pass scroll state
    ) -> Result<()> {
        let total_area = f.area();
        let input_height = (textarea.lines().len() as u16 + 2) // +2 for borders
            .min(MAX_INPUT_HEIGHT) // Clamp max height
            .min(total_area.height); // Clamp to available height

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(input_height)])
            .split(f.area());

        let messages_area = chunks[0];
        let input_area = chunks[1];

        // Render Messages
        Self::render_messages_static(f, messages_area, messages, scroll_offset, max_scroll);

        // Render Text Area
        f.render_widget(textarea, input_area);

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

    // Render Messages - Static method (mostly unchanged, uses static scroll)
    fn render_messages_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        messages: &[FormattedMessage],
        scroll_offset: usize, // Receive scroll state
        max_scroll: usize,    // Receive scroll state
    ) {
        if messages.is_empty() {
            let placeholder =
                Paragraph::new("No messages yet...").style(Style::default().fg(Color::DarkGray));
            f.render_widget(placeholder, area);
            return;
        }

        // Flatten message content lines for calculating total height and rendering
        let all_lines: Vec<Line> = messages.iter().flat_map(|fm| fm.content.clone()).collect();
        let total_lines_count = all_lines.len();

        let area_height = area.height.saturating_sub(2) as usize; // Subtract borders

        // Max scroll is now passed in, no need to calculate/set here
        // let max_scroll = if total_lines_count > area_height {
        //     total_lines_count.saturating_sub(area_height)
        // } else {
        //     0
        // };
        // Self::set_max_scroll(max_scroll); // Setter removed

        // Use passed-in scroll offset, already clamped by the setter
        // let scroll_offset = Self::get_scroll_offset().min(max_scroll);
        // Ensure scroll_offset is updated if it was clamped - handled by setter
        // if scroll_offset != Self::get_scroll_offset() {
        //     Self::set_scroll_offset(scroll_offset);
        // }

        // Create list items from the flattened lines, applying scroll offset manually via slicing
        let start = scroll_offset.min(total_lines_count.saturating_sub(1)); // Ensure start is within bounds
        let end = (scroll_offset + area_height).min(total_lines_count); // Calculate end index, clamp to total lines
        let visible_items: Vec<ListItem> = if start < end {
            all_lines[start..end]
                .iter()
                .cloned()
                .map(ListItem::new)
                .collect()
        } else {
            Vec::new() // Handle case where scroll offset is beyond content
        };

        let messages_list =
            List::new(visible_items) // Use the sliced items
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

    // Handle user input keys -> returns action to dispatch or None
    async fn handle_input(&mut self, key: KeyEvent) -> Result<Option<InputAction>> {
        let mut action = None; // Action to be returned

        // --- Calculate Page Scroll Amount ---
        // Calculate half page height for Ctrl+U/D scrolling
        let half_page_height = self
            .terminal
            .size()?
            .height
            .saturating_sub(self.textarea.lines().len() as u16 + 2) // Input area height approx.
            .saturating_sub(2) // Message area borders
            .saturating_div(2) as usize; // Half the message area height

        // Calculate full page height for PageUp/PageDown
        let full_page_height = half_page_height * 2; // More accurate full page

        // --- Global/Mode-Independent Shortcuts ---
        match key.code {
            // Quit (only in Normal mode, moved down for clarity)
            // KeyCode::Char('q') | KeyCode::Char('Q') if self.input_mode == InputMode::Normal => {
            //     action = Some(InputAction::Exit);
            // }

            // Deny Tool (only in Approval mode)
            KeyCode::Esc if self.input_mode == InputMode::AwaitingApproval => {
                // Treat Esc as Deny in Approval mode
                if let Some((id, _, _)) = self.approval_request.take() {
                    action = Some(InputAction::DenyTool(id));
                }
                self.input_mode = InputMode::Normal; // Revert mode
                self.progress_message = None; // Clear progress
                return Ok(action); // Return early as mode changed
            }
            _ => {} // Other keys handled per mode or below
        }

        // --- Scrolling (Available in Normal and Editing, but NOT AwaitingApproval) ---
        if self.input_mode != InputMode::AwaitingApproval {
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
                KeyCode::Char('d') if self.input_mode == InputMode::Normal => {
                    if key.modifiers == KeyModifiers::CONTROL || key.modifiers == KeyModifiers::NONE
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
                }

                // Toggle Tool Result Truncation
                KeyCode::Char('t') | KeyCode::Char('T') => {
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
                            debug(
                                "tui.handle_input",
                                "No tool message found to toggle truncation",
                            );
                        }
                    }
                    // No return Ok(None) here as we might set an action
                }
                _ => {} // Other keys ignored in this block
            }
        } // End of `if self.input_mode != InputMode::AwaitingApproval`

        // --- Mode-specific handling ---
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
                    // Handle Ctrl+S as stop editing (alternative to Esc)
                    Input {
                        key: Key::Char('s'),
                        ctrl: true,
                        ..
                    }
                    | Input {
                        key: Key::Char('S'),
                        ctrl: true,
                        ..
                    } => {
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
                        // Start editing via Ctrl+S
                        KeyCode::Char('s') | KeyCode::Char('S')
                            if key.modifiers == KeyModifiers::CONTROL =>
                        {
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
                        // Quit
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            action = Some(InputAction::Exit);
                        }
                        _ => {} // Other keys ignored in Normal mode unless handled globally/as scrolling
                    }
                }
            }
            InputMode::AwaitingApproval => {
                // Only handle approval keys here if no action already set
                if action.is_none() {
                    match key.code {
                        // Approve Normal
                        KeyCode::Char('y') | KeyCode::Char('Y')
                            if key.modifiers == KeyModifiers::NONE =>
                        {
                            if let Some((id, _, _)) = self.approval_request.take() {
                                action = Some(InputAction::ApproveToolNormal(id));
                            }
                            self.input_mode = InputMode::Normal; // Revert mode
                            self.progress_message = None; // Clear progress
                        }
                        // Approve Always (Shift + Tab)
                        KeyCode::BackTab if key.modifiers == KeyModifiers::SHIFT => {
                            // Shift+Tab often comes as BackTab + Shift
                            if let Some((id, _, _)) = self.approval_request.take() {
                                action = Some(InputAction::ApproveToolAlways(id));
                            }
                            self.input_mode = InputMode::Normal; // Revert mode
                            self.progress_message = None; // Clear progress
                        }
                        // Deny
                        KeyCode::Char('n') | KeyCode::Char('N') /* Esc handled globally */ => {
                            if let Some((id, _, _)) = self.approval_request.take() {
                                action = Some(InputAction::DenyTool(id));
                            }
                            self.input_mode = InputMode::Normal; // Revert mode
                            self.progress_message = None; // Clear progress
                        }
                        _ => {} // Ignore other keys in this mode
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
                                    "{}\n... ({} more lines, press 't' to toggle full view)",
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
                        warn(
                            "tui.dispatch",
                            &format!(
                                "Message {} found, but has no full_tool_result to toggle.",
                                id
                            ),
                        );
                    }
                }
                if !found_and_toggled {
                    warn(
                        "tui.dispatch",
                        &format!("ToggleMessageTruncation: No message found with ID {}", id),
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
            }
            InputAction::ApproveToolAlways(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: true,
                        always: true,
                    })
                    .await?;
            }
            InputAction::DenyTool(id) => {
                self.command_tx
                    .send(AppCommand::HandleToolResponse {
                        id,
                        approved: false,
                        always: false,
                    })
                    .await?;
            }
            InputAction::Exit => {
                // Signal exit cleanly if possible
                if let Err(e) = self.command_tx.send(AppCommand::Shutdown).await {
                    error(
                        "tui.dispatch",
                        &format!("Failed to send Shutdown command: {}", e),
                    );
                    // Still exit the TUI loop even if shutdown command fails
                }
                return Ok(true); // Signal exit
            }
        }
        Ok(false) // Don't exit by default
    }

    // Close method (optional, Drop handles cleanup)
    // pub async fn close(&mut self) -> Result<()> {
    //     self.cleanup_terminal()?;
    //     Ok(())
    // }
}

// Implement Drop to ensure terminal cleanup happens
impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup_terminal() {
            // Log error if cleanup fails, but don't panic in drop
            eprintln!("Failed to cleanup terminal: {}", e);
            error("Tui::drop", &format!("Failed to cleanup terminal: {}", e));
        }
    }
}
