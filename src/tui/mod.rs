use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

mod message_formatter;

use message_formatter::format_message;

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

        Ok(Self {
            terminal,
            input: String::new(),
            input_mode: InputMode::Normal,
            messages: Vec::new(),
            scroll_offset: 0,
            is_processing: false,
        })
    }

    pub async fn run(&mut self, app: &mut crate::app::App) -> Result<()> {
        // Welcome message
        self.add_system_message("Welcome to Claude Code! Type your query and press Enter to send.");
        self.add_system_message("Press Ctrl+C to exit, Ctrl+S to toggle input mode.");

        // Add the system prompt to the conversation
        let system_prompt = crate::api::messages::create_system_prompt(app.environment_info());
        app.conversation
            .add_system_message(system_prompt.content.clone());

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
        }

        // Restore terminal
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .context("Failed to leave alternate screen")?;
        self.terminal.show_cursor()?;

        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        // Create copies of the data we need to use in the closure
        let messages = self.messages.clone();
        let input = self.input.clone();
        let input_mode = match self.input_mode {
            InputMode::Normal => false,
            InputMode::Editing => true,
        };

        self.terminal.draw(|f| {
            // Create main layout
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                .split(f.size());

            // Create message area
            let message_area = chunks[0];

            // Create input area
            let input_area = chunks[1];

            // Render messages (using cloned data)
            Self::render_messages_static(f, message_area, &messages);

            // Render input (using cloned data)
            Self::render_input_static(f, input_area, &input, input_mode);
        })?;

        Ok(())
    }

    // Static version of render_messages that doesn't borrow self
    fn render_messages_static(
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        messages: &[FormattedMessage],
    ) {
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

        let messages_list = List::new(messages_list)
            .block(Block::default().borders(Borders::ALL).title("Messages"))
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

    fn render_messages(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        // Create a list of messages
        let messages: Vec<ListItem> = self
            .messages
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

        let messages_list = List::new(messages)
            .block(Block::default().borders(Borders::ALL).title("Messages"))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");

        f.render_widget(messages_list, area);
    }

    fn render_input(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let input_style = match self.input_mode {
            InputMode::Normal => Style::default(),
            InputMode::Editing => Style::default().fg(Color::Yellow),
        };

        let input = Paragraph::new(ratatui::text::Text::from(self.input.as_str()))
            .style(input_style)
            .block(Block::default().borders(Borders::ALL).title("Input"));

        f.render_widget(input, area);

        // Show cursor in editing mode
        if let InputMode::Editing = self.input_mode {
            f.set_cursor(area.x + 1 + self.input.chars().count() as u16, area.y + 1);
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

    async fn send_message(&mut self, message: String, app: &mut crate::app::App) -> Result<()> {
        // Add user message to app and UI
        app.add_user_message(message.clone());
        self.add_user_message(&message);

        // Set processing flag
        self.is_processing = true;
        self.draw()?;

        // Special command handling
        if message.starts_with("/") {
            let response = app.handle_command(&message).await?;
            self.add_system_message(&response);
            self.is_processing = false;
            return Ok(());
        }

        // Processing indicator
        self.add_system_message("Thinking...");

        // Create a placeholder for assistant message
        app.add_assistant_message(String::new());
        self.add_assistant_message("");

        // Get tools
        let tools = Some(crate::api::tools::Tool::all());

        // Get a response from Claude (with streaming)
        let mut stream = app.get_claude_response_streaming(Some(&tools.as_ref().unwrap()));

        // Process the stream
        let mut response = String::new();

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

        // Now that we have the complete response, let's check for tool calls
        // For now, we'll just check for patterns in the text
        // In the future, the API client should properly detect tool calls in streaming responses
        if response.contains("<tool_use>")
            || response.contains("<function_calls>")
            || response.contains("<function_calls>")
        {
            self.add_system_message("Tool calls detected - processing");

            // Get a non-streaming response to properly parse tool calls
            // This is just a temporary solution until proper streaming tool call detection is implemented
            let resp = app
                .get_claude_response(Some(&tools.as_ref().unwrap()))
                .await?;

            if resp.has_tool_calls() {
                let tool_calls = resp.extract_tool_calls();

                // Execute all tool calls
                for tool_call in &tool_calls {
                    self.add_system_message(&format!("Executing tool: {}", tool_call.name));

                    match app.execute_tool(tool_call).await {
                        Ok(result) => {
                            // Add tool result to the conversation
                            app.conversation.add_message(
                                crate::app::Role::Tool,
                                format!("Tool result from {}: {}", tool_call.name, result),
                            );

                            // Display tool result in the UI
                            self.add_system_message(&format!(
                                "Tool {} executed successfully",
                                tool_call.name
                            ));
                        }
                        Err(e) => {
                            self.add_system_message(&format!(
                                "Error executing tool {}: {}",
                                tool_call.name, e
                            ));
                        }
                    }
                }

                // Continue the conversation with the tool results
                self.add_system_message("Continuing conversation with tool results...");

                // Get another response from Claude including the tool results
                let mut continuation_stream =
                    app.get_claude_response_streaming(Some(&tools.as_ref().unwrap()));

                // Add a placeholder for the new assistant message
                app.add_assistant_message(String::new());
                self.add_assistant_message("");

                // Process the continuation response
                let mut continuation_response = String::new();

                while let Some(chunk) = continuation_stream.next().await {
                    match chunk {
                        Ok(text) => {
                            // Update the message
                            continuation_response.push_str(&text);

                            // Update the last message in the UI
                            if let Some(last) = self.messages.last_mut() {
                                let formatted = format_message(
                                    &continuation_response,
                                    crate::app::Role::Assistant,
                                );
                                last.content = formatted;
                            }

                            // Update the last message in the app
                            if let Some(last) = app.conversation.messages.last_mut() {
                                last.content = continuation_response.clone();
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
            }
        }

        self.is_processing = false;
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
