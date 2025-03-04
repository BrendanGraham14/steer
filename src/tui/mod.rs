use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use std::io;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

mod message_formatter;

use message_formatter::format_message;

// UI States
enum InputMode {
    Normal,
    Editing,
}

// Static data to allow access from static methods
static SCROLL_OFFSET: RwLock<usize> = RwLock::new(0);
static MAX_SCROLL: RwLock<usize> = RwLock::new(0);
static SPINNER_STATE: RwLock<usize> = RwLock::new(0);

pub struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    input: String,
    input_mode: InputMode,
    messages: Vec<FormattedMessage>,
    is_processing: bool,
    // Progress tracking
    progress_message: Option<String>,
    last_spinner_update: std::time::Instant,
    // Shared messages (updated by event handler)
    shared_messages: Option<Arc<Mutex<Vec<FormattedMessage>>>>,
}

#[derive(Clone)]
pub struct FormattedMessage {
    content: Vec<Line<'static>>,
    role: crate::app::Role,
}

impl Tui {
    pub fn new() -> Result<Self> {
        // Setup terminal
        enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Failed to create terminal")?;

        // Reset scroll values
        if let Ok(mut offset) = SCROLL_OFFSET.write() {
            *offset = 0;
        }
        if let Ok(mut max) = MAX_SCROLL.write() {
            *max = 0;
        }
        
        // Reset spinner state
        if let Ok(mut spinner) = SPINNER_STATE.write() {
            *spinner = 0;
        }
        
        Ok(Self {
            terminal,
            input: String::new(),
            input_mode: InputMode::Normal,
            messages: Vec::new(),
            is_processing: false,
            progress_message: None,
            last_spinner_update: std::time::Instant::now(),
            shared_messages: None,
        })
    }

    pub async fn run(&mut self, app: &mut crate::app::App, mut event_rx: mpsc::Receiver<crate::app::AppEvent>) -> Result<()> {
        // Spawn a task to handle events from the app - do this first to set up shared message state
        let mut event_handle = self.spawn_event_handler(event_rx);
        
        // Welcome message
        self.add_system_message("Welcome to Claude Code! Type your query and press Enter to send.");
        self.add_system_message("Press Ctrl+C to exit, Ctrl+S to toggle input mode.");

        // Sync messages to the shared state
        if let Some(shared) = &self.shared_messages {
            // Update the shared messages
            if let Ok(mut messages) = shared.try_lock() {
                *messages = self.messages.clone();
                crate::utils::logging::info("tui.run", 
                    &format!("Initialized shared messages with welcome messages. Count: {}", messages.len()));
            }
        }

        // Add the system prompt to the conversation
        let system_prompt = if app.has_memory_file() {
            // Use the memory-enhanced system prompt
            crate::api::messages::create_system_prompt_with_memory(
                app.environment_info(), 
                app.memory_content()
            )
        } else {
            // Use the regular system prompt
            crate::api::messages::create_system_prompt(app.environment_info())
        };
        
        app.add_system_message(system_prompt.content.clone());

        loop {
            // Draw UI
            self.draw()?;

            // Handle input
            if crossterm::event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_input(key, app).await? {
                        break;
                    }
                }
            }
            
            // Check if the event handler has exited
            if event_handle.is_finished() {
                // Recreate the event handler if it has exited
                event_rx = app.setup_event_channel();
                event_handle = self.spawn_event_handler(event_rx);
            }
        }

        // Restore terminal
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .context("Failed to leave alternate screen")?;
        self.terminal.show_cursor()?;

        Ok(())
    }
    
    // Spawn a task to handle events from the app
    fn spawn_event_handler(&mut self, mut event_rx: mpsc::Receiver<crate::app::AppEvent>) -> JoinHandle<()> {
        // Clone the messages Vec to move into the task
        let messages = Arc::new(Mutex::new(self.messages.clone()));
        
        // Create a shared messages reference that will be updated by the event handler
        // and can be retrieved by the main TUI instance
        let shared_messages = Arc::clone(&messages);
        self.shared_messages = Some(shared_messages);
        
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let mut messages = messages.lock().await;
                
                // Update the progress message based on events
                match &event {
                    crate::app::AppEvent::ThinkingStarted => {
                        if let Ok(mut state) = SPINNER_STATE.write() {
                            *state = 0; // Reset spinner state
                        }
                    }
                    crate::app::AppEvent::ToolCallStarted { name: _ } => {
                        if let Ok(mut state) = SPINNER_STATE.write() {
                            *state = 0; // Reset spinner state
                        }
                    }
                    _ => {}
                }
                
                match event {
                    crate::app::AppEvent::MessageAdded { role, content } => {
                        // Check if we have a matching message already (for updates)
                        let mut found = false;
                        for msg in messages.iter_mut() {
                            if msg.role == role {
                                // Update the existing message
                                msg.content = format_message(&content, role); // Role now implements Copy
                                found = true;
                                break;
                            }
                        }
                        
                        // If not found, add a new message
                        if !found {
                            let formatted = format_message(&content, role); // Role now implements Copy
                            messages.push(FormattedMessage {
                                content: formatted,
                                role,
                            });
                            
                            // Debug info with proper logging
                            let content_summary = if content.len() > 50 { 
                                format!("{}...", &content[..50]) 
                            } else { 
                                content.clone() 
                            };
                            crate::utils::logging::debug("tui.event_handler", 
                                &format!("Added new message to shared state. Role: {:?}, Content summary: {:?}", 
                                     role, content_summary));
                            crate::utils::logging::debug("tui.event_handler", 
                                &format!("Messages count in shared: {}", messages.len()));
                            
                            // Reset scroll to show most recent content for new messages
                            // (not for updates to existing messages)
                            Tui::set_scroll_offset(0);
                        }
                    },
                    crate::app::AppEvent::ToolCallStarted { name } => {
                        let formatted = format_message(&format!("Starting tool call: {}", name), crate::app::Role::System);
                        messages.push(FormattedMessage {
                            content: formatted,
                            role: crate::app::Role::System,
                        });
                    },
                    crate::app::AppEvent::ToolCallCompleted { name, result: _ } => {
                        let formatted = format_message(&format!("Tool {} executed successfully", name), crate::app::Role::System);
                        messages.push(FormattedMessage {
                            content: formatted,
                            role: crate::app::Role::System,
                        });
                    },
                    crate::app::AppEvent::ToolCallFailed { name, error } => {
                        let formatted = format_message(&format!("Tool {} failed: {}", name, error), crate::app::Role::System);
                        messages.push(FormattedMessage {
                            content: formatted,
                            role: crate::app::Role::System,
                        });
                    },
                    crate::app::AppEvent::ThinkingStarted => {
                        let formatted = format_message("Thinking...", crate::app::Role::System);
                        messages.push(FormattedMessage {
                            content: formatted,
                            role: crate::app::Role::System,
                        });
                    },
                    crate::app::AppEvent::ThinkingCompleted => {
                        // No need to add a message for this
                    },
                    crate::app::AppEvent::CommandResponse { content } => {
                        let formatted = format_message(&content, crate::app::Role::System);
                        messages.push(FormattedMessage {
                            content: formatted,
                            role: crate::app::Role::System,
                        });
                    },
                    crate::app::AppEvent::Error { message } => {
                        let formatted = format_message(&format!("Error: {}", message), crate::app::Role::System);
                        messages.push(FormattedMessage {
                            content: formatted,
                            role: crate::app::Role::System,
                        });
                    },
                }
            }
        })
    }

    // Static helper functions for scroll management
    fn get_scroll_offset() -> usize {
        SCROLL_OFFSET.read().map(|offset| *offset).unwrap_or(0)
    }
    
    fn set_scroll_offset(offset: usize) {
        if let Ok(mut scroll) = SCROLL_OFFSET.write() {
            *scroll = offset;
        }
    }
    
    fn get_max_scroll() -> usize {
        MAX_SCROLL.read().map(|max| *max).unwrap_or(0)
    }
    
    fn set_max_scroll(max: usize) {
        if let Ok(mut max_scroll) = MAX_SCROLL.write() {
            *max_scroll = max;
        }
    }
    
    // Get current spinner character
    fn get_spinner_char() -> &'static str {
        const SPINNER_CHARS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let state = SPINNER_STATE.read().map(|s| *s).unwrap_or(0) % SPINNER_CHARS.len();
        SPINNER_CHARS[state]
    }
    
    // Update spinner state
    fn update_spinner_state(&mut self) {
        let now = std::time::Instant::now();
        // Update spinner every 80ms
        if now.duration_since(self.last_spinner_update).as_millis() >= 80 {
            if let Ok(mut state) = SPINNER_STATE.write() {
                *state = (*state + 1) % 10;
            }
            self.last_spinner_update = now;
        }
    }
    
    // This function isn't needed after our refactoring
    // fn update_shared_messages(&mut self, shared: Arc<Mutex<Vec<FormattedMessage>>>) {
    //     self.shared_messages = Some(shared);
    // }
    
    // Sync messages from the shared state if available
    fn sync_messages(&mut self) {
        if let Some(shared) = &self.shared_messages {
            // Try to acquire the lock with a timeout - using try_lock with a retry strategy
            let max_attempts = 5;
            let mut attempts = 0;
            let mut success = false;
            
            while attempts < max_attempts && !success {
                if let Ok(messages) = shared.try_lock() {
                    // Debug with proper logging
                    crate::utils::logging::debug("tui.sync_messages", 
                        &format!("Syncing messages from shared state. Count: {}", messages.len()));
                    // Replace our messages with the latest from the shared state
                    self.messages = messages.clone();
                    crate::utils::logging::debug("tui.sync_messages", 
                        &format!("Local message count after sync: {}", self.messages.len()));
                    success = true;
                } else {
                    // Small delay before retry
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    attempts += 1;
                }
            }
            
            if !success {
                crate::utils::logging::warn("tui.sync_messages", 
                    &format!("Failed to acquire lock for syncing messages after {} attempts", max_attempts));
            }
        } else {
            crate::utils::logging::warn("tui.sync_messages", 
                "No shared messages reference available");
        }
    }
    
    fn draw(&mut self) -> Result<()> {
        // Update spinner if we're processing
        if self.is_processing {
            self.update_spinner_state();
        }
        
        // Sync messages from shared state before drawing
        self.sync_messages();
        
        // Debug to check messages before rendering
        crate::utils::logging::debug("tui.draw", &format!("Drawing UI with {} messages", self.messages.len()));
        
        // Create copies of the data we need to use in the closure
        let messages = self.messages.clone();
        let input = self.input.clone();
        let input_mode = match self.input_mode {
            InputMode::Normal => false,
            InputMode::Editing => true,
        };
        let is_processing = self.is_processing;
        let progress_message = self.progress_message.clone();
        
        // Update max scroll value based on current message content and terminal size
        let terminal_height = self.terminal.size()?.height as usize;
        let visible_lines = terminal_height.saturating_sub(5); // Adjust for borders and input area
        let total_lines = messages.iter().map(|m| m.content.len() + 1).sum::<usize>();
        let max_scroll = total_lines.saturating_sub(visible_lines).max(0);
        Self::set_max_scroll(max_scroll);
        
        // Ensure scroll offset is within bounds
        let current_offset = Self::get_scroll_offset();
        if current_offset > max_scroll {
            Self::set_scroll_offset(max_scroll);
        }

        self.terminal.draw(|f| {
            // Create main layout
            let mut constraints = vec![Constraint::Min(1), Constraint::Length(3)];
            
            // Add progress area if processing
            if is_processing {
                constraints.insert(1, Constraint::Length(1));
            }
            
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(f.size());

            // Create message area
            let message_area = chunks[0];

            // Create progress area if processing
            if is_processing {
                // Render progress indicator
                let spinner_char = Self::get_spinner_char();
                let progress_text = if let Some(msg) = progress_message {
                    format!("{} {}", spinner_char, msg)
                } else {
                    format!("{} Processing...", spinner_char)
                };
                
                let progress_widget = Paragraph::new(progress_text)
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(progress_widget, chunks[1]);
                
                // Render input (using cloned data)
                Self::render_input_static(f, chunks[2], &input, input_mode);
            } else {
                // Render input without progress area
                let input_area = chunks[1];
                Self::render_input_static(f, input_area, &input, input_mode);
            }

            // Render messages (using cloned data)
            Self::render_messages_static(f, message_area, &messages);
        })?;

        Ok(())
    }

    // Static version of render_messages that doesn't borrow self
    fn render_messages_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        messages: &[FormattedMessage],
    ) {
        crate::utils::logging::debug("tui.render_messages", &format!("Rendering messages. Count: {}", messages.len()));
        for (i, msg) in messages.iter().enumerate() {
            crate::utils::logging::debug("tui.render_messages", 
                &format!("  Message {}: Role {:?}, Content lines: {}", i, msg.role, msg.content.len()));
        }
        
        let scroll_offset = Tui::get_scroll_offset();
        
        // Create a list of messages
        let messages_list: Vec<ListItem> = messages
            .iter()
            .map(|m| {
                let header_style = match m.role {
                    crate::app::Role::User => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    crate::app::Role::Assistant => Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                    crate::app::Role::System => Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    crate::app::Role::Tool => Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                };

                // Create header
                let header = Line::from(Span::styled(format!("[ {} ]", m.role), header_style));

                // Create a list item with content
                let mut lines = vec![header];
                lines.extend(m.content.clone());
                ListItem::new(lines)
            })
            .collect();

        // Calculate max scroll
        let max_scroll = if messages.is_empty() {
            0
        } else {
            // Rough calculation of the total lines in all messages
            let total_lines = messages.iter().map(|m| m.content.len() + 1).sum::<usize>();
            // Subtract visible lines (approximation)
            let visible_lines = area.height as usize - 2; // Account for borders
            total_lines.saturating_sub(visible_lines)
        };

        // Set the title with scroll indicator if needed
        let title = if max_scroll > 0 {
            format!("Messages [{}/{}]", scroll_offset, max_scroll)
        } else {
            "Messages".to_string()
        };

        // Create list with items based on scroll offset
        let offset = scroll_offset as usize;
        let visible_messages: Vec<ListItem> = if messages_list.len() > offset {
            messages_list.into_iter().skip(offset).collect()
        } else {
            Vec::new()
        };
        
        let messages_list = List::new(visible_messages)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");

        f.render_widget(messages_list, area);
    }

    // Static version of render_input that doesn't borrow self
    fn render_input_static(f: &mut ratatui::Frame<'_>, area: Rect, input: &str, is_editing: bool) {
        let input_style = if is_editing {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let paragraph = Paragraph::new(ratatui::text::Text::from(input))
            .style(input_style)
            .block(Block::default().borders(Borders::ALL).title("Input"));

        f.render_widget(paragraph, area);

        // Show cursor in editing mode
        if is_editing {
            // Get the input string length for cursor positioning
            let string_len = input.chars().count() as u16;
            f.set_cursor(area.x + 1 + string_len, area.y + 1);
        }
    }

    async fn handle_input(&mut self, key: KeyEvent, app: &mut crate::app::App) -> Result<bool> {
        match self.input_mode {
            InputMode::Normal => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(true);
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input_mode = InputMode::Editing;
                }
                KeyCode::Char('i') => {
                    self.input_mode = InputMode::Editing;
                }
                KeyCode::Up => {
                    let current = Self::get_scroll_offset();
                    if current > 0 {
                        Self::set_scroll_offset(current - 1);
                    }
                }
                KeyCode::Down => {
                    let current = Self::get_scroll_offset();
                    let max = Self::get_max_scroll();
                    if current < max {
                        Self::set_scroll_offset(current + 1);
                    }
                }
                KeyCode::PageUp => {
                    let current = Self::get_scroll_offset();
                    let page_size = 10;
                    if current > page_size {
                        Self::set_scroll_offset(current - page_size);
                    } else {
                        Self::set_scroll_offset(0);
                    }
                }
                KeyCode::PageDown => {
                    let current = Self::get_scroll_offset();
                    let max = Self::get_max_scroll();
                    let page_size = 10;
                    if current + page_size < max {
                        Self::set_scroll_offset(current + page_size);
                    } else {
                        Self::set_scroll_offset(max);
                    }
                }
                KeyCode::Home => {
                    Self::set_scroll_offset(0);
                }
                KeyCode::End => {
                    let max = Self::get_max_scroll();
                    Self::set_scroll_offset(max);
                }
                _ => {}
            },
            InputMode::Editing => match key.code {
                KeyCode::Enter => {
                    let message = self.input.drain(..).collect::<String>();
                    if !message.is_empty() {
                        self.send_message(message, app).await?;
                    }
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    if c == 'c' && key.modifiers.contains(KeyModifiers::CONTROL) {
                        return Ok(true);
                    } else {
                        self.input.push(c);
                    }
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                }
                _ => {}
            },
        }

        Ok(false)
    }

    // Set progress message
    fn set_progress(&mut self, message: Option<String>) {
        self.progress_message = message;
    }
    
    async fn send_message(&mut self, message: String, app: &mut crate::app::App) -> Result<()> {
        // Set processing flag and message
        self.is_processing = true;
        self.set_progress(Some("Sending message to Claude...".to_string()));
        self.draw()?;
        
        // Let the app process the message
        // The app will handle adding messages to the conversation
        // and emitting events to update the UI
        match app.process_user_message(message.clone()).await {
            Ok(_) => {},
            Err(e) => {
                // Force update UI with error message if API call failed
                self.add_system_message(&format!("Error: {}", e));
                self.draw()?;
                
                // Reset processing flag and message
                self.is_processing = false;
                self.set_progress(None);
                return Err(e);
            }
        }
        
        // Try one more UI refresh to make sure we display updated messages
        self.sync_messages();
        self.draw()?;
        
        // Reset processing flag and message
        self.is_processing = false;
        self.set_progress(None);
        
        // Reset scroll to follow newest messages
        Self::set_scroll_offset(0);
        
        // Draw once more after processing is complete
        self.draw()?;
        
        Ok(())
    }

    fn add_user_message(&mut self, content: &str) {
        let formatted = format_message(content, crate::app::Role::User);
        self.messages.push(FormattedMessage {
            content: formatted,
            role: crate::app::Role::User,
        });
    }

    fn add_assistant_message(&mut self, content: &str) {
        let formatted = format_message(content, crate::app::Role::Assistant);
        self.messages.push(FormattedMessage {
            content: formatted,
            role: crate::app::Role::Assistant,
        });
    }

    fn add_system_message(&mut self, content: &str) {
        let formatted = format_message(content, crate::app::Role::System);
        self.messages.push(FormattedMessage {
            content: formatted,
            role: crate::app::Role::System,
        });
    }
    
    fn add_tool_message(&mut self, content: &str) {
        let formatted = format_message(content, crate::app::Role::Tool);
        self.messages.push(FormattedMessage {
            content: formatted,
            role: crate::app::Role::Tool,
        });
    }
}
