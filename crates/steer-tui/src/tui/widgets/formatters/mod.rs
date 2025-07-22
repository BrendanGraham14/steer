use crate::tui::theme::Theme;
use lazy_static::lazy_static;
use ratatui::text::Line;
use serde_json::Value;
use std::collections::HashMap;
use steer_core::app::conversation::ToolResult;

pub mod astgrep;
pub mod bash;
pub mod default;
pub mod dispatch_agent;
pub mod edit;
pub mod external;
pub mod fetch;
pub mod glob;
pub mod grep;
pub mod helpers;
pub mod ls;
pub mod replace;
pub mod todo_read;
pub mod todo_write;
pub mod view;

// Import the formatters
use self::astgrep::AstGrepFormatter;
use self::bash::BashFormatter;
use self::default::DefaultFormatter;
use self::dispatch_agent::DispatchAgentFormatter;
use self::edit::EditFormatter;
use self::external::ExternalFormatter;
use self::fetch::FetchFormatter;
use self::glob::GlobFormatter;
use self::grep::GrepFormatter;
use self::ls::LsFormatter;
use self::replace::ReplaceFormatter;

use self::todo_read::TodoReadFormatter;
use self::todo_write::TodoWriteFormatter;
use self::view::ViewFormatter;

/// Trait for formatting tool calls and results
pub trait ToolFormatter: Send + Sync {
    /// Format tool call and result in compact mode (single line summary)
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>>;

    /// Format tool call and result in detailed mode (full parameters and output)
    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>>;

    /// Format tool call for approval request (shows what the tool will do)
    fn approval(&self, params: &Value, wrap_width: usize, theme: &Theme) -> Vec<Line<'static>> {
        // Default implementation just wraps compact() without result
        self.compact(params, &None, wrap_width, theme)
    }
}

lazy_static! {
    static ref FORMATTERS: HashMap<&'static str, Box<dyn ToolFormatter>> = {
        use steer_tools::tools::*;

        let mut map: HashMap<&'static str, Box<dyn ToolFormatter>> = HashMap::new();

        map.insert(BASH_TOOL_NAME, Box::new(BashFormatter));
        map.insert(GREP_TOOL_NAME, Box::new(GrepFormatter));
        map.insert(LS_TOOL_NAME, Box::new(LsFormatter));
        map.insert(GLOB_TOOL_NAME, Box::new(GlobFormatter));
        map.insert(VIEW_TOOL_NAME, Box::new(ViewFormatter));
        map.insert(EDIT_TOOL_NAME, Box::new(EditFormatter));
        map.insert(edit::multi_edit::MULTI_EDIT_TOOL_NAME, Box::new(EditFormatter)); // Multi-edit uses same formatter
        map.insert(REPLACE_TOOL_NAME, Box::new(ReplaceFormatter));
        map.insert(TODO_READ_TOOL_NAME, Box::new(TodoReadFormatter));
        map.insert(TODO_WRITE_TOOL_NAME, Box::new(TodoWriteFormatter));
        map.insert(AST_GREP_TOOL_NAME, Box::new(AstGrepFormatter));
        map.insert(steer_core::tools::fetch::FETCH_TOOL_NAME, Box::new(FetchFormatter));
        map.insert(steer_core::tools::dispatch_agent::DISPATCH_AGENT_TOOL_NAME, Box::new(DispatchAgentFormatter));

        // Catch-all formatter for external/MCP tools (name prefix "mcp__") will be handled in get_formatter

        map
    };

    static ref DEFAULT_FORMATTER: Box<dyn ToolFormatter> = Box::new(DefaultFormatter);
    static ref EXTERNAL_FORMATTER: Box<dyn ToolFormatter> = Box::new(ExternalFormatter);
}

pub fn get_formatter(tool_name: &str) -> &'static dyn ToolFormatter {
    if let Some(fmt) = FORMATTERS.get(tool_name) {
        fmt.as_ref()
    } else if tool_name.starts_with("mcp__") {
        EXTERNAL_FORMATTER.as_ref()
    } else {
        DEFAULT_FORMATTER.as_ref()
    }
}
