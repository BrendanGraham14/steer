//! TUI module for the steer CLI
//!
//! This module implements the terminal user interface using ratatui.

use std::collections::{HashSet, VecDeque};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;

const IMAGE_TOKEN_LABEL_PREFIX: &str = "[Image: ";
const IMAGE_TOKEN_LABEL_SUFFIX: &str = "]";
const FIRST_ATTACHMENT_TOKEN: u32 = 0xE000;

#[derive(Debug, Clone)]
struct PendingAttachment {
    image: ImageContent,
    token: char,
}

use crate::tui::update::UpdateStatus;

use crate::error::{Error, Result};
use crate::notifications::{NotificationManager, NotificationManagerHandle};
use crate::tui::commands::registry::CommandRegistry;
use crate::tui::model::{ChatItem, NoticeLevel, TuiCommandResponse};
use crate::tui::theme::Theme;
use futures::{FutureExt, StreamExt};
use image::ImageFormat;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, EventStream, KeyCode, KeyEventKind, MouseEvent};
use ratatui::{Frame, Terminal};
use steer_grpc::AgentClient;
use steer_grpc::client_api::{
    AssistantContent, ClientEvent, EditingMode, ImageContent, ImageSource, LlmStatus, Message,
    MessageData, ModelId, OpId, Preferences, ProviderId, UserContent, WorkspaceStatus, builtin,
    default_primary_agent_id,
};

use crate::tui::events::processor::PendingToolApproval;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

fn auth_status_from_source(
    source: Option<&steer_grpc::client_api::AuthSource>,
) -> crate::tui::state::AuthStatus {
    match source {
        Some(steer_grpc::client_api::AuthSource::ApiKey { .. }) => {
            crate::tui::state::AuthStatus::ApiKeySet
        }
        Some(steer_grpc::client_api::AuthSource::Plugin { .. }) => {
            crate::tui::state::AuthStatus::OAuthConfigured
        }
        _ => crate::tui::state::AuthStatus::NotConfigured,
    }
}

fn has_any_auth_source(source: Option<&steer_grpc::client_api::AuthSource>) -> bool {
    matches!(
        source,
        Some(
            steer_grpc::client_api::AuthSource::ApiKey { .. }
                | steer_grpc::client_api::AuthSource::Plugin { .. }
        )
    )
}

fn format_inline_image_token(mime_type: &str) -> String {
    format!("{IMAGE_TOKEN_LABEL_PREFIX}{mime_type}{IMAGE_TOKEN_LABEL_SUFFIX}")
}

fn attachment_spans(content: &str, attachments: &[PendingAttachment]) -> Vec<(char, usize, usize)> {
    let mut spans = Vec::new();

    for (start, ch) in content.char_indices() {
        let Some(attachment) = attachments.iter().find(|attachment| attachment.token == ch) else {
            continue;
        };

        let mut end = start + ch.len_utf8();
        let label = format_inline_image_token(&attachment.image.mime_type);
        if content[end..].starts_with(&label) {
            end += label.len();
        }

        spans.push((attachment.token, start, end));
    }

    spans
}

fn parse_inline_message_content(content: &str, images: &[PendingAttachment]) -> Vec<UserContent> {
    if images.is_empty() {
        let trimmed = content.trim().to_string();
        if trimmed.is_empty() {
            return Vec::new();
        }
        return vec![UserContent::Text { text: trimmed }];
    }

    let mut result = Vec::new();
    let mut text_buf = String::new();
    let mut cursor = 0;

    while cursor < content.len() {
        let ch = match content[cursor..].chars().next() {
            Some(ch) => ch,
            None => break,
        };

        if let Some(attachment) = images.iter().find(|attachment| attachment.token == ch) {
            let trimmed = text_buf.trim().to_string();
            if !trimmed.is_empty() {
                result.push(UserContent::Text { text: trimmed });
            }
            text_buf.clear();
            result.push(UserContent::Image {
                image: attachment.image.clone(),
            });

            cursor += ch.len_utf8();
            let label = format_inline_image_token(&attachment.image.mime_type);
            if content[cursor..].starts_with(&label) {
                cursor += label.len();
            }
            continue;
        }

        text_buf.push(ch);
        cursor += ch.len_utf8();
    }

    let trimmed = text_buf.trim().to_string();
    if !trimmed.is_empty() {
        result.push(UserContent::Text { text: trimmed });
    }

    result
}

fn strip_image_token_labels(content: &str) -> String {
    let mut output = String::new();
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        let ch_u32 = ch as u32;
        if !(FIRST_ATTACHMENT_TOKEN..=0xF8FF).contains(&ch_u32) {
            output.push(ch);
            i += 1;
            continue;
        }

        output.push(ch);
        i += 1;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }

        let mut j = i;
        while j < chars.len() && chars[j] != ']' {
            j += 1;
        }

        if j < chars.len() {
            let candidate: String = chars[i..=j].iter().collect();
            if candidate.starts_with(IMAGE_TOKEN_LABEL_PREFIX)
                && candidate.ends_with(IMAGE_TOKEN_LABEL_SUFFIX)
            {
                i = j + 1;
            }
        }
    }

    output
}

fn decode_pasted_image(data: &str) -> Option<ImageContent> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let format = image::guess_format(&bytes).ok()?;

    let mime_type = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Bmp => "image/bmp",
        ImageFormat::Tiff => "image/tiff",
        _ => return None,
    }
    .to_string();

    Some(ImageContent {
        source: ImageSource::DataUrl {
            data_url: format!("data:{};base64,{}", mime_type, data),
        },
        mime_type,
        width: None,
        height: None,
        bytes: Some(bytes.len() as u64),
        sha256: None,
    })
}

fn encode_clipboard_rgba_image(
    width: usize,
    height: usize,
    rgba_bytes: &[u8],
) -> Option<ImageContent> {
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    let rgba = image::RgbaImage::from_raw(width, height, rgba_bytes.to_vec())?;

    let mut png_cursor = io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png_cursor, ImageFormat::Png)
        .ok()?;

    let png_bytes = png_cursor.into_inner();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    Some(ImageContent {
        source: ImageSource::DataUrl {
            data_url: format!("data:image/png;base64,{encoded}"),
        },
        mime_type: "image/png".to_string(),
        width: Some(width),
        height: Some(height),
        bytes: u64::try_from(png_bytes.len()).ok(),
        sha256: None,
    })
}

pub(crate) fn format_agent_label(primary_agent_id: &str) -> String {
    let agent_id = if primary_agent_id.is_empty() {
        default_primary_agent_id()
    } else {
        primary_agent_id
    };
    agent_id.to_string()
}

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
use crate::tui::widgets::EditSelectionOverlayState;
use crate::tui::widgets::InputPanel;
use crate::tui::widgets::input_panel::InputPanelParams;
use tracing::error as tracing_error;
use tracing::info as tracing_info;

pub mod commands;
pub mod custom_commands;
pub mod model;
pub mod state;
pub mod terminal;
pub mod theme;
pub mod widgets;

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
const SCROLL_FLUSH_INTERVAL: Duration = Duration::from_millis(16);
const MOUSE_SCROLL_STEP: usize = 1;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollDirection {
    Up,
    Down,
}

impl ScrollDirection {
    fn from_mouse_event(event: &MouseEvent) -> Option<Self> {
        match event.kind {
            event::MouseEventKind::ScrollUp => Some(Self::Up),
            event::MouseEventKind::ScrollDown => Some(Self::Down),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingScroll {
    direction: ScrollDirection,
    steps: usize,
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
    /// Pending image attachments to include on next send.
    pending_attachments: Vec<PendingAttachment>,
    next_attachment_token: u32,
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
    /// Current primary agent label for status bar
    current_agent_label: Option<String>,
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
    /// Track in-flight operations (operation_id -> chat_store_index)
    in_flight_operations: HashSet<OpId>,
    /// Queued head item (if any)
    queued_head: Option<steer_grpc::client_api::QueuedWorkItem>,
    /// Count of queued items
    queued_count: usize,
    /// Command registry for slash commands
    command_registry: CommandRegistry,
    /// User preferences
    preferences: Preferences,
    /// Centralized notification manager
    notification_manager: NotificationManagerHandle,
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
    edit_selection_state: EditSelectionOverlayState,
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
            EditingMode::Simple => InputMode::Simple,
            EditingMode::Vim => InputMode::VimNormal,
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

    fn has_pending_send_content(&self) -> bool {
        self.input_panel_state.has_content() || !self.pending_attachments.is_empty()
    }

    fn next_attachment_token(&mut self) -> Option<char> {
        let max = 0x0010_FFFF;
        while self.next_attachment_token <= max {
            let candidate = self.next_attachment_token;
            self.next_attachment_token += 1;

            let Some(token) = char::from_u32(candidate) else {
                continue;
            };
            if token.is_control() || token == '\n' || token == '\r' {
                continue;
            }
            if !((FIRST_ATTACHMENT_TOKEN..=0xF8FF).contains(&(token as u32))) {
                continue;
            }
            if self
                .pending_attachments
                .iter()
                .any(|attachment| attachment.token == token)
            {
                continue;
            }
            return Some(token);
        }

        None
    }

    fn remove_attachment_for_token(&mut self, token: char) {
        if let Some(index) = self
            .pending_attachments
            .iter()
            .position(|attachment| attachment.token == token)
        {
            self.pending_attachments.remove(index);
        }
    }

    fn add_pending_attachment(&mut self, image: ImageContent) {
        let Some(token) = self.next_attachment_token() else {
            warn!(target: "tui.input", "Ran out of attachment token characters");
            self.push_notice(
                NoticeLevel::Warn,
                "Unable to attach more images in this input.".to_string(),
            );
            return;
        };

        let mime_type = image.mime_type.clone();
        self.pending_attachments
            .push(PendingAttachment { image, token });
        self.input_panel_state.textarea.insert_char(token);
        self.input_panel_state
            .textarea
            .insert_str(&format_inline_image_token(&mime_type));
    }

    fn cursor_position_from_byte_offset(content: &str, byte_offset: usize) -> (u16, u16) {
        let offset = byte_offset.min(content.len());
        let mut row = 0usize;
        let mut col = 0usize;

        for ch in content[..offset].chars() {
            if ch == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }

        (
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(col).unwrap_or(u16::MAX),
        )
    }

    fn replace_input_content_with_cursor_offset(&mut self, content: &str, cursor_offset: usize) {
        let cursor = Self::cursor_position_from_byte_offset(content, cursor_offset);
        self.input_panel_state
            .replace_content(content, Some(cursor));
    }

    fn replace_input_content_preserving_cursor(&mut self, content: &str) {
        let current_content = self.input_panel_state.content();
        let cursor_offset = self
            .input_panel_state
            .get_cursor_byte_offset()
            .min(current_content.len())
            .min(content.len());
        self.replace_input_content_with_cursor_offset(content, cursor_offset);
    }

    fn sync_attachments_from_input_tokens(&mut self) {
        let content = self.input_panel_state.content();

        if self.pending_attachments.is_empty() {
            let stripped = strip_image_token_labels(&content);
            if stripped != content {
                self.replace_input_content_preserving_cursor(&stripped);
            }
            return;
        }

        let mut normalized = String::new();
        let mut cursor = 0usize;
        let mut retained_tokens: HashSet<char> = HashSet::new();

        while cursor < content.len() {
            let Some(ch) = content[cursor..].chars().next() else {
                break;
            };

            if let Some(attachment) = self
                .pending_attachments
                .iter()
                .find(|attachment| attachment.token == ch)
            {
                let label = format_inline_image_token(&attachment.image.mime_type);
                retained_tokens.insert(attachment.token);
                normalized.push(ch);
                normalized.push_str(&label);

                cursor += ch.len_utf8();
                if content[cursor..].starts_with(&label) {
                    cursor += label.len();
                } else if content[cursor..].starts_with(IMAGE_TOKEN_LABEL_PREFIX)
                    && let Some(label_end) = content[cursor..].find(']')
                {
                    cursor += label_end + 1;
                }
                continue;
            }

            normalized.push(ch);
            cursor += ch.len_utf8();
        }

        self.pending_attachments
            .retain(|attachment| retained_tokens.contains(&attachment.token));

        let normalized = if self.pending_attachments.is_empty() {
            strip_image_token_labels(&normalized)
        } else {
            normalized
        };

        if normalized != content {
            self.replace_input_content_preserving_cursor(&normalized);
        }
    }

    fn handle_atomic_backspace_delete(&mut self, delete_forward: bool) -> bool {
        if self.pending_attachments.is_empty() {
            return false;
        }

        let content = self.input_panel_state.content();
        let cursor_offset = self
            .input_panel_state
            .get_cursor_byte_offset()
            .min(content.len());

        let target_offset = if delete_forward {
            if cursor_offset >= content.len() {
                return false;
            }
            cursor_offset
        } else {
            if cursor_offset == 0 {
                return false;
            }

            match content[..cursor_offset].char_indices().next_back() {
                Some((idx, _)) => idx,
                None => return false,
            }
        };

        let Some((token, start, end)) = attachment_spans(&content, &self.pending_attachments)
            .into_iter()
            .find(|(_, start, end)| (*start..*end).contains(&target_offset))
        else {
            return false;
        };

        let mut next_content = String::new();
        next_content.push_str(&content[..start]);
        next_content.push_str(&content[end..]);

        self.remove_attachment_for_token(token);
        self.replace_input_content_with_cursor_offset(&next_content, start);
        true
    }

    fn try_attach_image_from_clipboard(&mut self) -> bool {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(clipboard) => clipboard,
            Err(err) => {
                debug!(target: "tui.input", "Clipboard unavailable for Ctrl+V: {err}");
                return false;
            }
        };

        let image = match clipboard.get_image() {
            Ok(image) => image,
            Err(err) => {
                debug!(target: "tui.input", "No clipboard image found for Ctrl+V: {err}");
                return false;
            }
        };

        if let Some(image_content) =
            encode_clipboard_rgba_image(image.width, image.height, image.bytes.as_ref())
        {
            self.add_pending_attachment(image_content);
            true
        } else {
            warn!(
                target: "tui.input",
                "Clipboard image had invalid dimensions: {}x{} ({} bytes)",
                image.width,
                image.height,
                image.bytes.len()
            );
            self.push_notice(
                NoticeLevel::Warn,
                "Clipboard image format is unsupported.".to_string(),
            );
            true
        }
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
        let preferences = Preferences::load()
            .map_err(|e| crate::error::Error::Config(e.to_string()))
            .unwrap_or_default();

        // Determine initial input mode based on editing mode preference
        let input_mode = match preferences.ui.editing_mode {
            EditingMode::Simple => InputMode::Simple,
            EditingMode::Vim => InputMode::VimNormal,
        };

        let notification_manager = std::sync::Arc::new(NotificationManager::new(&preferences));

        let mut tui = Self {
            terminal,
            terminal_size,
            input_mode,
            input_panel_state: crate::tui::widgets::input_panel::InputPanelState::new(
                session_id.clone(),
            ),
            editing_message_id: None,
            pending_attachments: Vec::new(),
            next_attachment_token: FIRST_ATTACHMENT_TOKEN,
            client,
            is_processing: false,
            progress_message: None,
            spinner_state: 0,
            current_tool_approval: None,
            current_model,
            current_agent_label: None,
            event_pipeline: Self::create_event_pipeline(notification_manager.clone()),
            chat_store: ChatStore::new(),
            tool_registry: ToolCallRegistry::new(),
            chat_viewport: ChatViewport::new(),
            session_id,
            theme: theme.unwrap_or_default(),
            setup_state: None,
            in_flight_operations: HashSet::new(),
            queued_head: None,
            queued_count: 0,
            command_registry: CommandRegistry::new(),
            preferences,
            notification_manager,
            double_tap_tracker: crate::tui::state::DoubleTapTracker::new(),
            vim_state: VimState::default(),
            mode_stack: VecDeque::new(),
            last_revision: 0,
            update_status: UpdateStatus::Checking,
            edit_selection_state: EditSelectionOverlayState::default(),
        };

        tui.refresh_agent_label().await;
        tui.notification_manager.set_focus_events_enabled(true);

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
            if let MessageData::Tool { tool_use_id, .. } = &message.data {
                debug!(
                    target: "tui.restore",
                    "Found Tool message with tool_use_id={}",
                    tool_use_id
                );
            }
        }

        self.chat_store.ingest_messages(&messages);
        if let Some(message) = messages.last() {
            self.chat_store
                .set_active_message_id(Some(message.id().to_string()));
        }

        // The rest of the tool registry population code remains the same
        // Extract tool calls from assistant messages
        for message in &messages {
            if let MessageData::Assistant { content, .. } = &message.data {
                debug!(
                    target: "tui.restore",
                    "Processing Assistant message id={}",
                    message.id()
                );
                for block in content {
                    if let AssistantContent::ToolCall { tool_call, .. } = block {
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
            if let MessageData::Tool { tool_use_id, .. } = &message.data {
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

    fn format_grpc_error(error: &steer_grpc::GrpcError) -> String {
        match error {
            steer_grpc::GrpcError::CallFailed(status) => status.message().to_string(),
            _ => error.to_string(),
        }
    }

    fn format_workspace_status(status: &WorkspaceStatus) -> String {
        let mut output = String::new();
        output.push_str(&format!("Workspace: {}\n", status.workspace_id.as_uuid()));
        output.push_str(&format!(
            "Environment: {}\n",
            status.environment_id.as_uuid()
        ));
        output.push_str(&format!("Repo: {}\n", status.repo_id.as_uuid()));
        output.push_str(&format!("Path: {}\n", status.path.display()));

        match &status.vcs {
            Some(vcs) => {
                output.push_str(&format!(
                    "VCS: {} ({})\n\n",
                    vcs.kind.as_str(),
                    vcs.root.display()
                ));
                output.push_str(&vcs.status.as_llm_string());
            }
            None => {
                output.push_str("VCS: <none>\n");
            }
        }

        output
    }

    async fn refresh_agent_label(&mut self) {
        match self.client.get_session(&self.session_id).await {
            Ok(Some(session)) => {
                if let Some(config) = session.config.as_ref() {
                    let agent_id = config
                        .primary_agent_id
                        .clone()
                        .unwrap_or_else(|| default_primary_agent_id().to_string());
                    self.current_agent_label = Some(format_agent_label(&agent_id));
                }
            }
            Ok(None) => {
                warn!(
                    target: "tui.session",
                    "No session data available to populate agent label"
                );
            }
            Err(e) => {
                warn!(
                    target: "tui.session",
                    "Failed to load session config for agent label: {}",
                    e
                );
            }
        }
    }

    async fn start_new_session(&mut self) -> Result<()> {
        use std::collections::HashMap;
        use steer_grpc::client_api::{
            CreateSessionParams, SessionPolicyOverrides, SessionToolConfig, WorkspaceConfig,
        };

        let session_params = CreateSessionParams {
            workspace: WorkspaceConfig::default(),
            tool_config: SessionToolConfig::default(),
            primary_agent_id: None,
            policy_overrides: SessionPolicyOverrides::empty(),
            metadata: HashMap::new(),
            default_model: self.current_model.clone(),
        };

        let new_session_id = self
            .client
            .create_session(session_params)
            .await
            .map_err(|e| Error::Generic(format!("Failed to create new session: {e}")))?;

        self.session_id.clone_from(&new_session_id);
        self.client.subscribe_session_events().await?;
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
        self.current_agent_label = None;
        self.refresh_agent_label().await;

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
        let mut pending_scroll: Option<PendingScroll> = None;

        // Create a tick interval for spinner updates
        let mut tick = tokio::time::interval(SPINNER_UPDATE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut scroll_flush = tokio::time::interval(SCROLL_FLUSH_INTERVAL);
        scroll_flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

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
                        Some(Ok(evt)) => {
                            let (event_needs_redraw, event_should_exit) = self
                                .handle_terminal_event(
                                    evt,
                                    term_event_stream,
                                    &mut pending_scroll,
                                    &mut scroll_flush,
                                )
                                .await?;
                            if event_needs_redraw {
                                needs_redraw = true;
                            }
                            if event_should_exit {
                                should_exit = true;
                            }
                        }
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

                    if self.input_mode == InputMode::Setup
                        && crate::tui::handlers::setup::SetupHandler::poll_oauth_callback(self)
                            .await?
                        {
                            needs_redraw = true;
                        }
                }
                _ = scroll_flush.tick(), if pending_scroll.is_some() => {
                    if let Some(pending) = pending_scroll.take()
                        && self.apply_scroll_steps(pending.direction, pending.steps) {
                            needs_redraw = true;
                        }
                }
            }
        }

        Ok(())
    }

    async fn handle_terminal_event(
        &mut self,
        event: Event,
        term_event_stream: &mut EventStream,
        pending_scroll: &mut Option<PendingScroll>,
        scroll_flush: &mut tokio::time::Interval,
    ) -> Result<(bool, bool)> {
        let mut needs_redraw = false;
        let mut should_exit = false;
        let mut pending_events = VecDeque::new();
        pending_events.push_back(event);

        while let Some(event) = pending_events.pop_front() {
            match event {
                Event::FocusGained => {
                    self.notification_manager.set_terminal_focused(true);
                }
                Event::FocusLost => {
                    self.notification_manager.set_terminal_focused(false);
                }
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    match self.handle_key_event(key_event).await {
                        Ok(exit) => {
                            if exit {
                                should_exit = true;
                            }
                        }
                        Err(e) => {
                            // Display error as a system notice
                            use crate::tui::model::{
                                ChatItem, ChatItemData, NoticeLevel, generate_row_id,
                            };
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
                    let (scroll_pending, mouse_needs_redraw, mouse_exit, deferred_event) =
                        self.handle_mouse_event_coalesced(mouse_event, term_event_stream)?;
                    if let Some(scroll) = scroll_pending {
                        let pending_was_empty = pending_scroll.is_none();
                        match pending_scroll {
                            Some(pending) if pending.direction == scroll.direction => {
                                pending.steps = pending.steps.saturating_add(scroll.steps);
                            }
                            _ => {
                                *pending_scroll = Some(scroll);
                            }
                        }
                        if pending_was_empty {
                            scroll_flush.reset_after(SCROLL_FLUSH_INTERVAL);
                        }
                    }
                    needs_redraw |= mouse_needs_redraw;
                    should_exit |= mouse_exit;
                    if let Some(deferred_event) = deferred_event {
                        pending_events.push_front(deferred_event);
                    }
                }
                Event::Resize(width, height) => {
                    self.terminal_size = (width, height);
                    // Terminal was resized, force redraw
                    needs_redraw = true;
                }
                Event::Paste(data) => {
                    if !self.is_text_input_mode() {
                        continue;
                    }

                    if self.input_mode == InputMode::Setup {
                        if let Some(setup_state) = &mut self.setup_state
                            && matches!(
                                &setup_state.current_step,
                                crate::tui::state::SetupStep::Authentication(_)
                            )
                        {
                            setup_state.auth_input.push_str(&data);
                            debug!(
                                target:"tui.run",
                                "Pasted {} chars in Setup mode",
                                data.len()
                            );
                            needs_redraw = true;
                        }
                        continue;
                    }

                    let maybe_image = decode_pasted_image(&data);
                    let had_image = maybe_image.is_some();
                    let normalized_data =
                        strip_image_token_labels(&data.replace("\r\n", "\n").replace('\r', "\n"));
                    let mut text_inserted = false;
                    if !normalized_data.is_empty() {
                        self.input_panel_state.insert_str(&normalized_data);
                        text_inserted = true;
                        debug!(
                            target:"tui.run",
                            "Pasted {} chars in {:?} mode",
                            normalized_data.len(),
                            self.input_mode
                        );
                    }

                    if let Some(image) = maybe_image {
                        self.add_pending_attachment(image);
                    }

                    if text_inserted || had_image {
                        needs_redraw = true;
                    }
                }
                Event::Key(_) => {}
            }

            if should_exit {
                break;
            }
        }

        Ok((needs_redraw, should_exit))
    }

    fn handle_mouse_event_coalesced(
        &mut self,
        mouse_event: MouseEvent,
        term_event_stream: &mut EventStream,
    ) -> Result<(Option<PendingScroll>, bool, bool, Option<Event>)> {
        let Some(mut last_direction) = ScrollDirection::from_mouse_event(&mouse_event) else {
            let needs_redraw = self.handle_mouse_event(mouse_event)?;
            return Ok((None, needs_redraw, false, None));
        };

        let mut steps = 1usize;
        let mut deferred_event = None;
        let mut should_exit = false;

        loop {
            let next_event = term_event_stream.next().now_or_never();
            let Some(next_event) = next_event else {
                break;
            };

            match next_event {
                Some(Ok(Event::Mouse(next_mouse))) => {
                    if let Some(next_direction) = ScrollDirection::from_mouse_event(&next_mouse) {
                        if next_direction == last_direction {
                            steps = steps.saturating_add(1);
                        } else {
                            last_direction = next_direction;
                            steps = 1;
                        }
                        continue;
                    }
                    deferred_event = Some(Event::Mouse(next_mouse));
                    break;
                }
                Some(Ok(other_event)) => {
                    deferred_event = Some(other_event);
                    break;
                }
                Some(Err(e)) => {
                    if e.kind() == io::ErrorKind::Interrupted {
                        debug!(target: "tui.input", "Ignoring interrupted syscall");
                    } else {
                        error!(target: "tui.run", "Fatal input error: {}. Exiting.", e);
                        should_exit = true;
                    }
                    break;
                }
                None => {
                    should_exit = true;
                    break;
                }
            }
        }

        Ok((
            Some(PendingScroll {
                direction: last_direction,
                steps,
            }),
            false,
            should_exit,
            deferred_event,
        ))
    }

    /// Handle mouse events
    fn handle_mouse_event(&mut self, event: MouseEvent) -> Result<bool> {
        let needs_redraw = match ScrollDirection::from_mouse_event(&event) {
            Some(direction) => self.apply_scroll_steps(direction, 1),
            None => false,
        };

        Ok(needs_redraw)
    }

    fn apply_scroll_steps(&mut self, direction: ScrollDirection, steps: usize) -> bool {
        // In vim normal mode or simple mode (when not typing), allow scrolling
        if !self.is_text_input_mode()
            || (self.input_mode == InputMode::Simple && self.input_panel_state.content().is_empty())
        {
            let amount = steps.saturating_mul(MOUSE_SCROLL_STEP);
            match direction {
                ScrollDirection::Up => self.chat_viewport.state_mut().scroll_up(amount),
                ScrollDirection::Down => self.chat_viewport.state_mut().scroll_down(amount),
            }
        } else {
            false
        }
    }

    /// Draw the UI
    fn draw(&mut self) -> Result<()> {
        let editing_message_id = self.editing_message_id.clone();
        let is_editing = editing_message_id.is_some();
        let editing_preview = if is_editing {
            self.editing_preview()
        } else {
            None
        };

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

            let queue_preview = self.queued_head.as_ref().map(|item| item.content.as_str());
            let input_area_height = self.input_panel_state.required_height(
                current_tool_call,
                terminal_size.width,
                terminal_size.height,
                queue_preview,
            );

            let layout = UiLayout::compute(terminal_size, input_area_height, &self.theme);
            layout.prepare_background(f, &self.theme);

            self.chat_viewport.rebuild(
                &chat_items,
                layout.chat.width,
                self.chat_viewport.state().view_mode,
                &self.theme,
                &self.chat_store,
                editing_message_id.as_deref(),
            );

            self.chat_viewport
                .render(f, layout.chat, spinner_state, None, &self.theme);

            let input_panel = InputPanel::new(InputPanelParams {
                input_mode,
                current_approval: current_tool_call,
                is_processing,
                spinner_state,
                is_editing,
                editing_preview: editing_preview.as_deref(),
                queued_count: self.queued_count,
                queued_preview: queue_preview,
                attachment_count: self.pending_attachments.len(),
                theme: &self.theme,
            });
            f.render_stateful_widget(input_panel, layout.input, &mut self.input_panel_state);

            let update_badge = match &self.update_status {
                UpdateStatus::Available(info) => {
                    crate::tui::widgets::status_bar::UpdateBadge::Available {
                        latest: &info.latest,
                    }
                }
                _ => crate::tui::widgets::status_bar::UpdateBadge::None,
            };
            layout.render_status_bar(
                f,
                &current_model_owned,
                self.current_agent_label.as_deref(),
                &self.theme,
                update_badge,
            );

            // Get fuzzy finder results before the render call
            let fuzzy_finder_data = if input_mode == InputMode::FuzzyFinder {
                let results = self.input_panel_state.fuzzy_finder.results().to_vec();
                let selected = self.input_panel_state.fuzzy_finder.selected_index();
                let input_height = self.input_panel_state.required_height(
                    current_tool_call,
                    terminal_size.width,
                    10,
                    queue_preview,
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

            if input_mode == InputMode::EditMessageSelection {
                use crate::tui::widgets::EditSelectionOverlay;
                let overlay = EditSelectionOverlay::new(&self.theme);
                f.render_stateful_widget(overlay, terminal_size, &mut self.edit_selection_state);
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
    fn create_event_pipeline(notification_manager: NotificationManagerHandle) -> EventPipeline {
        EventPipeline::new()
            .add_processor(Box::new(ProcessingStateProcessor::new(
                notification_manager.clone(),
            )))
            .add_processor(Box::new(MessageEventProcessor::new()))
            .add_processor(Box::new(
                crate::tui::events::processors::queue::QueueEventProcessor::new(),
            ))
            .add_processor(Box::new(ToolEventProcessor::new(
                notification_manager.clone(),
            )))
            .add_processor(Box::new(SystemEventProcessor::new(notification_manager)))
    }

    fn preprocess_client_event_double_tap(
        event: &ClientEvent,
        double_tap_tracker: &mut crate::tui::state::DoubleTapTracker,
    ) {
        if matches!(
            event,
            ClientEvent::OperationCancelled {
                popped_queued_item: Some(_),
                ..
            }
        ) {
            // Cancelling with queued work restores that draft into input; clear ESC
            // tap state so the second keypress doesn't immediately wipe it.
            double_tap_tracker.clear_key(&KeyCode::Esc);
        }
    }

    async fn handle_client_event(&mut self, event: ClientEvent) {
        Self::preprocess_client_event_double_tap(&event, &mut self.double_tap_tracker);
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
            notification_manager: &self.notification_manager,
            input_panel_state: &mut self.input_panel_state,
            is_processing: &mut self.is_processing,
            progress_message: &mut self.progress_message,
            spinner_state: &mut self.spinner_state,
            current_tool_approval: &mut self.current_tool_approval,
            current_model: &mut self.current_model,
            current_agent_label: &mut self.current_agent_label,
            messages_updated: &mut messages_updated,
            in_flight_operations: &mut self.in_flight_operations,
            queued_head: &mut self.queued_head,
            queued_count: &mut self.queued_count,
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
            self.chat_viewport.mark_dirty();
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
            self.chat_viewport.mark_dirty();
            if content.starts_with('!') && content.len() > 1 {
                let command = content[1..].trim().to_string();
                if let Err(e) = self.client.execute_bash_command(command).await {
                    self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
                }
            } else if let Err(e) = self
                .client
                .edit_message(message_id_to_edit, content, self.current_model.clone())
                .await
            {
                self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
            }
            return Ok(());
        }

        let content_blocks = parse_inline_message_content(&content, &self.pending_attachments);

        if let Err(e) = self
            .client
            .send_content_message(content_blocks, self.current_model.clone())
            .await
        {
            self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
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

        if let Some(cmd_info) = self.command_registry.get(cmd_name)
            && let crate::tui::commands::registry::CommandScope::Custom(custom_cmd) =
                &cmd_info.scope
        {
            // Create a TuiCommand::Custom and process it
            let app_cmd = TuiAppCommand::Tui(TuiCommand::Custom(custom_cmd.clone()));
            // Process through the normal flow
            match app_cmd {
                TuiAppCommand::Tui(TuiCommand::Custom(custom_cmd)) => {
                    // Handle custom command based on its type
                    match custom_cmd {
                        crate::tui::custom_commands::CustomCommand::Prompt { prompt, .. } => {
                            self.client
                                .send_message(prompt, self.current_model.clone())
                                .await?;
                        }
                    }
                }
                _ => unreachable!(),
            }
            return Ok(());
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

                        let mut status_map = std::collections::HashMap::new();
                        for s in statuses {
                            status_map.insert(s.provider_id.clone(), s.auth_source);
                        }

                        // Convert remote providers into a minimal registry-like view for TUI
                        let registry =
                            std::sync::Arc::new(RemoteProviderRegistry::from_proto(providers));

                        for p in registry.all() {
                            let status = auth_status_from_source(
                                status_map.get(&p.id).and_then(|s| s.as_ref()),
                            );
                            provider_status.insert(ProviderId(p.id.clone()), status);
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
                                self.preferences.ui.editing_mode = EditingMode::Simple;
                                self.set_mode(InputMode::Simple);
                                self.preferences
                                    .save()
                                    .map_err(|e| crate::error::Error::Config(e.to_string()))?;
                                "Switched to Simple mode".to_string()
                            }
                            Some("vim") => {
                                self.preferences.ui.editing_mode = EditingMode::Vim;
                                self.set_mode(InputMode::VimNormal);
                                self.preferences
                                    .save()
                                    .map_err(|e| crate::error::Error::Config(e.to_string()))?;
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
                    TuiCommand::Workspace(ref workspace_id) => {
                        let target_id = if let Some(workspace_id) = workspace_id.clone() {
                            Some(workspace_id)
                        } else {
                            let session = if let Some(session) =
                                self.client.get_session(&self.session_id).await?
                            {
                                session
                            } else {
                                self.push_notice(
                                    NoticeLevel::Error,
                                    "Session not found for workspace status".to_string(),
                                );
                                return Ok(());
                            };
                            let config = if let Some(config) = session.config {
                                config
                            } else {
                                self.push_notice(
                                    NoticeLevel::Error,
                                    "Session config missing for workspace status".to_string(),
                                );
                                return Ok(());
                            };
                            config.workspace_id.or_else(|| {
                                config.workspace_ref.map(|reference| reference.workspace_id)
                            })
                        };

                        let target_id = match target_id {
                            Some(id) if !id.is_empty() => id,
                            _ => {
                                self.push_notice(
                                    NoticeLevel::Error,
                                    "Workspace id not available for current session".to_string(),
                                );
                                return Ok(());
                            }
                        };

                        match self.client.get_workspace_status(&target_id).await {
                            Ok(status) => {
                                let response = Self::format_workspace_status(&status);
                                self.push_tui_response(
                                    tui_cmd.as_command_str(),
                                    TuiCommandResponse::Text(response),
                                );
                            }
                            Err(e) => {
                                self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
                            }
                        }
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
            TuiAppCommand::Core(core_cmd) => match core_cmd {
                crate::tui::core_commands::CoreCommandType::Compact => {
                    if let Err(e) = self
                        .client
                        .compact_session(self.current_model.clone())
                        .await
                    {
                        self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
                    }
                }
                crate::tui::core_commands::CoreCommandType::Agent { target } => {
                    if let Some(agent_id) = target {
                        if let Err(e) = self.client.switch_primary_agent(agent_id.clone()).await {
                            self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
                        }
                    } else {
                        self.push_notice(NoticeLevel::Error, "Usage: /agent <mode>".to_string());
                    }
                }
                crate::tui::core_commands::CoreCommandType::Model { target } => {
                    if let Some(model_name) = target {
                        match self.client.resolve_model(&model_name).await {
                            Ok(model_id) => {
                                self.current_model = model_id;
                            }
                            Err(e) => {
                                self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
                            }
                        }
                    }
                }
            },
        }

        Ok(())
    }

    /// Enter edit mode for a specific message
    fn enter_edit_mode(&mut self, message_id: &str) {
        // Find the message in the store
        if let Some(item) = self.chat_store.get_by_id(&message_id.to_string())
            && let crate::tui::model::ChatItemData::Message(message) = &item.data
            && let MessageData::User { content, .. } = &message.data
        {
            // Extract text content from user blocks
            let text = content
                .iter()
                .filter_map(|block| match block {
                    UserContent::Text { text } => Some(text.as_str()),
                    UserContent::Image { .. } | UserContent::CommandExecution { .. } => {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            // Set up textarea with the message content
            self.input_panel_state
                .set_content_from_lines(text.lines().collect::<Vec<_>>());
            // Switch to appropriate mode based on editing preference
            self.input_mode = match self.preferences.ui.editing_mode {
                EditingMode::Simple => InputMode::Simple,
                EditingMode::Vim => InputMode::VimInsert,
            };

            // Store the message ID we're editing
            self.editing_message_id = Some(message_id.to_string());
            self.chat_viewport.mark_dirty();
        }
    }

    fn cancel_edit_mode(&mut self) {
        if self.editing_message_id.is_some() {
            self.editing_message_id = None;
            self.chat_viewport.mark_dirty();
        }
    }

    fn editing_preview(&self) -> Option<String> {
        const EDIT_PREVIEW_MAX_LEN: usize = 40;

        let message_id = self.editing_message_id.as_ref()?;
        let item = self.chat_store.get_by_id(message_id)?;
        let crate::tui::model::ChatItemData::Message(message) = &item.data else {
            return None;
        };

        let content = message.content_string();
        let preview_line = content
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        if preview_line.is_empty() {
            return None;
        }

        let mut chars = preview_line.chars();
        let mut preview: String = chars.by_ref().take(EDIT_PREVIEW_MAX_LEN).collect();
        if chars.next().is_some() {
            preview.push('…');
        }

        Some(preview)
    }

    fn enter_edit_selection_mode(&mut self) {
        self.switch_mode(InputMode::EditMessageSelection);
        let messages = self.chat_store.user_messages_in_lineage();
        self.edit_selection_state.populate(messages);
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
        tracing_error!("Application panicked:");
        tracing_error!("{panic_info}");
    }));
}

/// High-level entry point for running the TUI
pub async fn run_tui(
    client: steer_grpc::AgentClient,
    session_id: Option<String>,
    model: ModelId,
    directory: Option<std::path::PathBuf>,
    theme_name: Option<String>,
    force_setup: bool,
) -> Result<()> {
    use std::collections::HashMap;
    use steer_grpc::client_api::{
        CreateSessionParams, SessionPolicyOverrides, SessionToolConfig, WorkspaceConfig,
    };

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
        let (messages, _approved_tools) =
            client.resume_session(&session_id).await.map_err(Box::new)?;
        info!(
            "Resumed session: {} with {} messages",
            session_id,
            messages.len()
        );
        tracing_info!("Session ID: {session_id}");
        (session_id, messages)
    } else {
        // Create a new session
        let workspace = if let Some(ref dir) = directory {
            WorkspaceConfig::Local { path: dir.clone() }
        } else {
            WorkspaceConfig::default()
        };
        let session_params = CreateSessionParams {
            workspace,
            tool_config: SessionToolConfig::default(),
            primary_agent_id: None,
            policy_overrides: SessionPolicyOverrides::empty(),
            metadata: HashMap::new(),
            default_model: model.clone(),
        };

        let session_id = client
            .create_session(session_params)
            .await
            .map_err(Box::new)?;
        (session_id, vec![])
    };

    client.subscribe_session_events().await.map_err(Box::new)?;
    let event_rx = client.subscribe_client_events().await.map_err(Box::new)?;
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
        tui.chat_viewport.state_mut().scroll_to_bottom();
    }

    // Query server for providers' auth status to decide if we should launch setup
    let statuses = tui
        .client
        .get_provider_auth_status(None)
        .await
        .map_err(|e| Error::Generic(format!("Failed to get provider auth status: {e}")))?;

    let has_any_auth = statuses
        .iter()
        .any(|s| has_any_auth_source(s.auth_source.as_ref()));

    let should_run_setup = force_setup
        || (!Preferences::config_path()
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
            status_map.insert(s.provider_id.clone(), s.auth_source);
        }

        let mut provider_status = std::collections::HashMap::new();
        for p in registry.all() {
            let status = auth_status_from_source(status_map.get(&p.id).and_then(|s| s.as_ref()));
            provider_status.insert(ProviderId(p.id.clone()), status);
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
        model.unwrap_or(builtin::claude_sonnet_4_5()),
        session_db,
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

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use steer_grpc::client_api::{AssistantContent, Message, MessageData, OpId, QueuedWorkItem};
    use tempfile::tempdir;

    /// RAII guard to ensure terminal state is restored after a test, even on panic.
    struct TerminalCleanupGuard;

    impl Drop for TerminalCleanupGuard {
        fn drop(&mut self) {
            cleanup();
        }
    }

    #[test]
    fn operation_cancelled_with_popped_queue_item_clears_esc_double_tap_tracker() {
        let mut tracker = crate::tui::state::DoubleTapTracker::new();
        tracker.record_key(KeyCode::Esc);

        let popped = QueuedWorkItem {
            kind: steer_grpc::client_api::QueuedWorkKind::UserMessage,
            content: "queued draft".to_string(),
            model: None,
            queued_at: 123,
            op_id: OpId::new(),
            message_id: steer_grpc::client_api::MessageId::from_string("msg_queued"),
        };

        Tui::preprocess_client_event_double_tap(
            &ClientEvent::OperationCancelled {
                op_id: OpId::new(),
                pending_tool_calls: 0,
                popped_queued_item: Some(popped),
            },
            &mut tracker,
        );

        assert!(
            !tracker.is_double_tap(KeyCode::Esc, Duration::from_millis(300)),
            "Esc tracker should be cleared when cancellation restores a queued item"
        );
    }

    #[test]
    fn operation_cancelled_without_popped_queue_item_keeps_esc_double_tap_tracker() {
        let mut tracker = crate::tui::state::DoubleTapTracker::new();
        tracker.record_key(KeyCode::Esc);

        Tui::preprocess_client_event_double_tap(
            &ClientEvent::OperationCancelled {
                op_id: OpId::new(),
                pending_tool_calls: 0,
                popped_queued_item: None,
            },
            &mut tracker,
        );

        assert!(
            tracker.is_double_tap(KeyCode::Esc, Duration::from_millis(300)),
            "Esc tracker should remain armed when cancellation does not restore queued input"
        );
    }

    #[tokio::test]
    #[ignore = "Requires TTY - run with `cargo test -- --ignored` in a terminal"]
    async fn test_ctrl_r_scrolls_to_bottom_in_simple_mode() {
        let _guard = TerminalCleanupGuard;
        let workspace_root = tempdir().expect("tempdir");
        let (client, _server_handle) =
            local_client_and_server(None, Some(workspace_root.path().to_path_buf())).await;
        let model = builtin::claude_sonnet_4_5();
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(client, model, session_id, None)
            .await
            .expect("create tui");

        tui.preferences.ui.editing_mode = EditingMode::Simple;
        tui.input_mode = InputMode::Simple;

        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        tui.handle_simple_mode(key).await.expect("handle ctrl+r");

        assert_eq!(
            tui.chat_viewport.state().view_mode,
            crate::tui::widgets::ViewMode::Detailed
        );
        assert_eq!(
            tui.chat_viewport.state_mut().take_scroll_target(),
            Some(crate::tui::widgets::ScrollTarget::Bottom)
        );
    }

    #[tokio::test]
    #[ignore = "Requires TTY - run with `cargo test -- --ignored` in a terminal"]
    async fn test_restore_messages_preserves_tool_call_params() {
        let _guard = TerminalCleanupGuard;
        // Create a TUI instance for testing
        let workspace_root = tempdir().expect("tempdir");
        let (client, _server_handle) =
            local_client_and_server(None, Some(workspace_root.path().to_path_buf())).await;
        let model = builtin::claude_sonnet_4_5();
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(client, model, session_id, None)
            .await
            .expect("create tui");

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
                    thought_signature: None,
                }],
            },
            id: "msg_assistant".to_string(),
            timestamp: 1_234_567_890,
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
            timestamp: 1_234_567_891,
            parent_message_id: Some("msg_assistant".to_string()),
        };

        let messages = vec![assistant_msg, tool_msg];

        // Restore messages
        tui.restore_messages(messages);

        // Verify tool call was preserved in registry
        if let Some(stored_call) = tui.tool_registry.get_tool_call(&tool_id) {
            assert_eq!(stored_call.name, "view");
            assert_eq!(stored_call.parameters, tool_call.parameters);
        } else {
            panic!("Tool call should be in registry");
        }
    }

    #[tokio::test]
    #[ignore = "Requires TTY - run with `cargo test -- --ignored` in a terminal"]
    async fn test_restore_messages_handles_tool_result_before_assistant() {
        let _guard = TerminalCleanupGuard;
        // Test edge case where Tool result arrives before Assistant message
        let workspace_root = tempdir().expect("tempdir");
        let (client, _server_handle) =
            local_client_and_server(None, Some(workspace_root.path().to_path_buf())).await;
        let model = builtin::claude_sonnet_4_5();
        let session_id = "test_session_id".to_string();
        let mut tui = Tui::new(client, model, session_id, None)
            .await
            .expect("create tui");

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
            timestamp: 1_234_567_890,
            parent_message_id: None,
        };

        let assistant_msg = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::ToolCall {
                    tool_call: tool_call.clone(),
                    thought_signature: None,
                }],
            },
            id: "msg_456".to_string(),
            timestamp: 1_234_567_891,
            parent_message_id: None,
        };

        let messages = vec![tool_msg, assistant_msg];

        tui.restore_messages(messages);

        // Should still have proper parameters
        if let Some(stored_call) = tui.tool_registry.get_tool_call(&tool_id) {
            assert_eq!(stored_call.parameters, real_params);
            assert_eq!(stored_call.name, "view");
        } else {
            panic!("Tool call should be in registry");
        }
    }

    #[test]
    fn strip_image_token_labels_removes_rendered_image_marker_text() {
        let content = format!("hello{IMAGE_TOKEN_CHAR}[Image: image/png] world");
        assert_eq!(strip_image_token_labels(&content), "hello world");
    }

    #[test]
    fn parse_inline_message_content_preserves_text_image_order() {
        let first = PendingAttachment {
            image: ImageContent {
                source: ImageSource::DataUrl {
                    data_url: "data:image/png;base64,AAAA".to_string(),
                },
                mime_type: "image/png".to_string(),
                width: Some(1),
                height: Some(1),
                bytes: Some(4),
                sha256: None,
            },
            token: 'A',
        };
        let second = PendingAttachment {
            image: ImageContent {
                source: ImageSource::DataUrl {
                    data_url: "data:image/jpeg;base64,BBBB".to_string(),
                },
                mime_type: "image/jpeg".to_string(),
                width: Some(1),
                height: Some(1),
                bytes: Some(4),
                sha256: None,
            },
            token: 'B',
        };

        let content = format!("before {} middle {} after", first.token, second.token);
        let parsed = parse_inline_message_content(&content, &[first.clone(), second.clone()]);

        assert_eq!(parsed.len(), 5);
        assert!(matches!(
            &parsed[0],
            UserContent::Text { text } if text == "before"
        ));
        assert!(matches!(
            &parsed[1],
            UserContent::Image { image } if image.mime_type == first.image.mime_type
        ));
        assert!(matches!(
            &parsed[2],
            UserContent::Text { text } if text == "middle"
        ));
        assert!(matches!(
            &parsed[3],
            UserContent::Image { image } if image.mime_type == second.image.mime_type
        ));
        assert!(matches!(
            &parsed[4],
            UserContent::Text { text } if text == "after"
        ));
    }

    #[test]
    fn parse_inline_message_content_skips_marker_labels_after_tokens() {
        let attachment = PendingAttachment {
            image: ImageContent {
                source: ImageSource::DataUrl {
                    data_url: "data:image/png;base64,AAAA".to_string(),
                },
                mime_type: "image/png".to_string(),
                width: Some(1),
                height: Some(1),
                bytes: Some(4),
                sha256: None,
            },
            token: 'A',
        };

        let content = format!(
            "look {}{} done",
            attachment.token,
            format_inline_image_token(&attachment.image.mime_type)
        );
        let parsed = parse_inline_message_content(&content, &[attachment.clone()]);

        assert_eq!(parsed.len(), 3);
        assert!(matches!(&parsed[1], UserContent::Image { .. }));
        assert!(matches!(
            &parsed[2],
            UserContent::Text { text } if text == "done"
        ));
    }

    #[test]
    fn decode_pasted_image_recognizes_png_base64_and_sets_metadata() {
        let png_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+X2N8AAAAASUVORK5CYII=";

        let image = decode_pasted_image(png_base64).expect("png should decode");

        assert_eq!(image.mime_type, "image/png");
        assert!(matches!(image.source, ImageSource::DataUrl { .. }));
        assert_eq!(image.width, None);
        assert_eq!(image.height, None);
        assert!(image.bytes.is_some());
    }

    #[test]
    fn decode_pasted_image_rejects_non_image_payload() {
        let not_image = base64::engine::general_purpose::STANDARD.encode("plain text");
        let decoded = decode_pasted_image(&not_image);
        assert!(decoded.is_none());
    }

    #[test]
    fn encode_clipboard_rgba_image_converts_to_png_data_url() {
        let rgba = [255_u8, 0, 0, 255];
        let image = encode_clipboard_rgba_image(1, 1, &rgba);

        assert!(image.is_some(), "expected clipboard image to encode");
        let image = match image {
            Some(image) => image,
            None => unreachable!("asserted Some above"),
        };

        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.width, Some(1));
        assert_eq!(image.height, Some(1));
        assert!(matches!(image.bytes, Some(bytes) if bytes > 0));
        assert!(matches!(image.source, ImageSource::DataUrl { .. }));
    }

    #[test]
    fn encode_clipboard_rgba_image_rejects_invalid_pixel_data() {
        let invalid_rgba = [255_u8, 0, 0];
        let image = encode_clipboard_rgba_image(1, 1, &invalid_rgba);
        assert!(image.is_none());
    }
}
