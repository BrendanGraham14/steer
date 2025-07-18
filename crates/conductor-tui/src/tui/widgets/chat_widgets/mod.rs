use conductor_core::app::conversation::{AppCommandType, CommandResponse, CompactResult};

pub mod chat_widget;
pub mod command_response;
pub mod gutter;
pub mod in_flight_operation;
pub mod message_widget;
pub mod row_widget;
pub mod slash_input;
pub mod system_notice;
pub mod tool_widget;

pub use chat_widget::{ChatWidget, DynamicChatWidget, HeightCache, ParagraphWidget};
pub use command_response::CommandResponseWidget;
pub use gutter::{GutterWidget, RoleGlyph};
pub use in_flight_operation::InFlightOperationWidget;
pub use message_widget::MessageWidget;
pub use row_widget::RowWidget;
pub use slash_input::SlashInputWidget;
pub use system_notice::SystemNoticeWidget;
pub use tool_widget::ToolWidget;

/// Helper function to get spinner character
pub fn get_spinner_char(state: usize) -> char {
    const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    SPINNER_FRAMES[state % SPINNER_FRAMES.len()]
}

/// Helper function to format AppCommandType
pub fn format_app_command(cmd: &AppCommandType) -> String {
    match cmd {
        AppCommandType::Model { target } => {
            if let Some(model) = target {
                format!("/model {model}")
            } else {
                "/model".to_string()
            }
        }
        AppCommandType::Compact => "/compact".to_string(),
        AppCommandType::Clear => "/clear".to_string(),
    }
}

/// Helper function to format CommandResponse
pub fn format_command_response(resp: &CommandResponse) -> String {
    match resp {
        CommandResponse::Text(text) => text.clone(),
        CommandResponse::Compact(result) => match result {
            CompactResult::Success(summary) => summary.clone(),
            CompactResult::Cancelled => "Compact cancelled.".to_string(),
            CompactResult::InsufficientMessages => "Not enough messages to compact.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_frames() {
        assert_eq!(get_spinner_char(0), '⠋');
        assert_eq!(get_spinner_char(1), '⠙');
        assert_eq!(get_spinner_char(10), '⠋'); // Should wrap around
    }

    #[test]
    fn test_format_helpers() {
        assert_eq!(format_app_command(&AppCommandType::Clear), "/clear");
        assert_eq!(format_app_command(&AppCommandType::Compact), "/compact");
        assert_eq!(
            format_app_command(&AppCommandType::Model {
                target: Some("gpt-4".to_string())
            }),
            "/model gpt-4"
        );
        assert_eq!(
            format_app_command(&AppCommandType::Model { target: None }),
            "/model"
        );

        assert_eq!(
            format_command_response(&CommandResponse::Text("Hello".to_string())),
            "Hello"
        );
        assert_eq!(
            format_command_response(&CommandResponse::Compact(CompactResult::Cancelled)),
            "Compact cancelled."
        );
    }
}
