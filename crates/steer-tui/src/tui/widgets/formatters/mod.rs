use crate::tui::theme::Theme;
use ratatui::text::Line;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;
use steer_grpc::client_api::ToolResult;

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
pub mod todo;
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

use self::todo::{TodoReadFormatter, TodoWriteFormatter};
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

static FORMATTERS: LazyLock<HashMap<&'static str, Box<dyn ToolFormatter>>> = LazyLock::new(|| {
    use steer_tools::tools::{
        AST_GREP_TOOL_NAME, BASH_TOOL_NAME, DISPATCH_AGENT_TOOL_NAME, EDIT_TOOL_NAME,
        FETCH_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME, REPLACE_TOOL_NAME,
        TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME, VIEW_TOOL_NAME, edit,
    };

    let mut map: HashMap<&'static str, Box<dyn ToolFormatter>> = HashMap::new();

    map.insert(BASH_TOOL_NAME, Box::new(BashFormatter));
    map.insert(GREP_TOOL_NAME, Box::new(GrepFormatter));
    map.insert(LS_TOOL_NAME, Box::new(LsFormatter));
    map.insert(GLOB_TOOL_NAME, Box::new(GlobFormatter));
    map.insert(VIEW_TOOL_NAME, Box::new(ViewFormatter));
    map.insert(EDIT_TOOL_NAME, Box::new(EditFormatter));
    map.insert(
        edit::multi_edit::MULTI_EDIT_TOOL_NAME,
        Box::new(EditFormatter),
    );
    map.insert(REPLACE_TOOL_NAME, Box::new(ReplaceFormatter));
    map.insert(TODO_READ_TOOL_NAME, Box::new(TodoReadFormatter));
    map.insert(TODO_WRITE_TOOL_NAME, Box::new(TodoWriteFormatter));
    map.insert(AST_GREP_TOOL_NAME, Box::new(AstGrepFormatter));
    map.insert(FETCH_TOOL_NAME, Box::new(FetchFormatter));
    map.insert(DISPATCH_AGENT_TOOL_NAME, Box::new(DispatchAgentFormatter));

    map
});

static DEFAULT_FORMATTER: LazyLock<Box<dyn ToolFormatter>> =
    LazyLock::new(|| Box::new(DefaultFormatter));
static EXTERNAL_FORMATTER: LazyLock<Box<dyn ToolFormatter>> =
    LazyLock::new(|| Box::new(ExternalFormatter));

pub fn get_formatter(tool_name: &str) -> &'static dyn ToolFormatter {
    if let Some(fmt) = FORMATTERS.get(tool_name) {
        fmt.as_ref()
    } else if tool_name.starts_with("mcp__") {
        EXTERNAL_FORMATTER.as_ref()
    } else {
        DEFAULT_FORMATTER.as_ref()
    }
}
