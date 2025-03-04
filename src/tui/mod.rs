use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use std::io;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;

mod message_formatter;
mod tool_handler;

use message_formatter::format_message;
use tool_handler::handle_tool_call;

// UI States
enum InputMode {
    Normal,
    Editing,
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    input: String,
    input_mode: InputMode,
    messages: Vec<FormattedMessage>,
    scroll_offset: usize,
    is_processing: bool,
}

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

        Ok(Self {
            terminal,
            input: String::new(),
            input_mode: InputMode::Normal,
            messages: Vec::new(),
            scroll_offset: 0,
            is_processing: false,
        })
    }

    pub async fn run(&mut self, app: &mut crate::app::App, api_client: crate::api::Client) -> Result<()> {
        // Welcome message
        self.add_system_message("Welcome to Claude Code! Type your query and press Enter to send.");
        self.add_system_message("Press Ctrl+C to exit, Ctrl+S to toggle input mode.");

        // Add the system prompt to the conversation
        let system_prompt = crate::api::messages::create_system_prompt(app.environment_info());
        app.conversation.add_system_message(system_prompt.content.clone());

        loop {
            // Draw UI
            self.draw()?;

            // Handle input
            if crossterm::event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_input(key, app, &api_client).await? {
                        break;
                    }
                }
            }
        }

        // Restore terminal
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen
        ).context("Failed to leave alternate screen")?;
        self.terminal.show_cursor()?;

        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        self.terminal.draw(|f| {
            // Create main layout
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(3),
                ].as_ref())
                .split(f.size());

            // Create message area
            let message_area = chunks[0];
            
            // Create input area
            let input_area = chunks[1];

            // Render messages
            self.render_messages(f, message_area);
            
            // Render input
            self.render_input(f, input_area);
        })?;

        Ok(())
    }

    fn render_messages<B: Backend>(&self, f: &mut ratatui::Frame<B>, area: Rect) {
        // Create a list of messages
        let messages: Vec<ListItem> = self.messages
            .iter()
            .map(|m| {
                let header_style = match m.role {
                    crate::app::Role::User => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    crate::app::Role::Assistant => Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                    crate::app::Role::System => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    crate::app::Role::Tool => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                };

                // Create header
                let header = Line::from(Span::styled(
                    format!("[ {} ]", m.role), 
                    header_style
                ));

                // Create a list item with content
                let mut lines = vec![header];
                lines.extend(m.content.clone());
                ListItem::new(lines)
            })
            .collect();

        let messages_list = List::new(messages)
            .block(Block::default().borders(Borders::ALL).title("Messages"))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");

        f.render_widget(messages_list, area);
    }

    fn render_input<B: Backend>(&self, f: &mut ratatui::Frame<B>, area: Rect) {
        let input_style = match self.input_mode {
            InputMode::Normal => Style::default(),
            InputMode::Editing => Style::default().fg(Color::Yellow),
        };

        let input = Paragraph::new(self.input.as_ref())
            .style(input_style)
            .block(Block::default().borders(Borders::ALL).title("Input"));

        f.render_widget(input, area);
        
        // Show cursor in editing mode
        if let InputMode::Editing = self.input_mode {
            f.set_cursor(
                area.x + 1 + self.input.chars().count() as u16,
                area.y + 1,
            );
        }
    }

    async fn handle_input(&mut self, key: KeyEvent, app: &mut crate::app::App, api_client: &crate::api::Client) -> Result<bool> {
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
                    if self.scroll_offset > 0 {
                        self.scroll_offset -= 1;
                    }
                }
                KeyCode::Down => {
                    self.scroll_offset += 1;
                }
                KeyCode::PageUp => {
                    if self.scroll_offset > 10 {
                        self.scroll_offset -= 10;
                    } else {
                        self.scroll_offset = 0;
                    }
                }
                KeyCode::PageDown => {
                    self.scroll_offset += 10;
                }
                _ => {}
            },
            InputMode::Editing => match key.code {
                KeyCode::Enter => {
                    let message = self.input.drain(..).collect::<String>();
                    if !message.is_empty() {
                        self.send_message(message, app, api_client).await?;
                    }
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    self.input.push(c);
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(true);
                }
                _ => {}
            },
        }

        Ok(false)
    }

    async fn send_message(&mut self, message: String, app: &mut crate::app::App, api_client: &crate::api::Client) -> Result<()> {
        // Add user message to app and UI
        app.add_user_message(message.clone());
        self.add_user_message(&message);
        
        // Set processing flag
        self.is_processing = true;
        self.draw()?;
        
        // Special command handling
        if message.starts_with("/") {
            self.handle_command(&message, app, api_client).await?;
            self.is_processing = false;
            return Ok(());
        }
        
        // Prepare messages for API
        let messages = crate::api::messages::convert_conversation(&app.conversation());
        let tools = Some(crate::api::tools::Tool::all());
        
        // Processing indicator
        self.add_system_message("Thinking...");
        
        // Get a response from Claude (with streaming)
        let mut stream = api_client.complete_streaming(messages, tools);
        
        // Collect initial response
        let mut response = String::new();
        let mut tool_calls = Vec::new();
        
        // Create a placeholder for assistant message
        app.add_assistant_message(String::new());
        self.add_assistant_message("");
        
        // Process the stream
        use futures_core::Stream;
        use futures_util::StreamExt;
        
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(text) => {
                    // Update the placeholder message
                    response.push_str(&text);
                    
                    // Update the last message in the UI
                    if let Some(last) = self.messages.last_mut() {
                        let formatted = format_message(&response, crate::app::Role::Assistant);
                        last.content = formatted;
                    }
                    
                    // Update the last message in the app
                    if let Some(last) = app.conversation.messages.last_mut() {
                        last.content = response.clone();
                    }
                    
                    // Redraw UI
                    self.draw()?;
                }
                Err(e) => {
                    self.add_system_message(&format!("Error: {}", e));
                    break;
                }
            }
        }
        
        // Process any tool calls in the message
        self.process_tool_calls(&response, app, api_client).await?;
        
        self.is_processing = false;
        Ok(())
    }
    
    async fn process_tool_calls(&mut self, message: &str, app: &mut crate::app::App, api_client: &crate::api::Client) -> Result<()> {
        // Extract tool calls
        // This is a simplified version - you would need to properly parse the message to find tool calls
        if message.contains("<function_calls>") {
            self.add_system_message("Processing tool calls...");
            
            // Here we would:
            // 1. Parse the message to extract tool calls
            // 2. Execute each tool
            // 3. Add the tool results back to the conversation
            // 4. Continue the conversation with Claude
            
            // This is simplified for this example
            self.add_system_message("Tool calls processed");
        }
        
        Ok(())
    }
    
    async fn handle_command(&mut self, command: &str, app: &mut crate::app::App, api_client: &crate::api::Client) -> Result<()> {
        match command {
            "/help" => {
                self.add_system_message("Available commands:
/help - Show this help message
/compact - Compact the conversation history
/clear - Clear the conversation history
/exit - Exit the application");
            }
            "/compact" => {
                self.add_system_message("Compacting conversation...");
                app.conversation.compact(api_client).await?;
                self.add_system_message("Conversation compacted");
            }
            "/clear" => {
                app.conversation.clear();
                self.messages.clear();
                self.add_system_message("Conversation cleared");
            }
            "/exit" => {
                return Ok(());
            }
            _ => {
                self.add_system_message(&format!("Unknown command: {}", command));
            }
        }
        
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
}