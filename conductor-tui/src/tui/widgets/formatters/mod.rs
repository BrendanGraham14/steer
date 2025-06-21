use crate::app::conversation::ToolResult;
use ratatui::text::Line;
use serde_json::Value;
use std::collections::HashMap;
use lazy_static::lazy_static;

pub mod helpers;
pub mod default;
pub mod bash;
pub mod grep;
pub mod ls;
pub mod glob;
pub mod view;
pub mod edit;
pub mod replace;
pub mod todo_read;
pub mod todo_write;
pub mod fetch;
pub mod astgrep;
pub mod dispatch_agent;

// Import the formatters
use self::bash::BashFormatter;
use self::grep::GrepFormatter;
use self::ls::LsFormatter;
use self::glob::GlobFormatter;
use self::view::ViewFormatter;
use self::edit::EditFormatter;
use self::replace::ReplaceFormatter;
use self::todo_read::TodoReadFormatter;
use self::todo_write::TodoWriteFormatter;
use self::fetch::FetchFormatter;
use self::astgrep::AstGrepFormatter;
use self::dispatch_agent::DispatchAgentFormatter;
use self::default::DefaultFormatter;

/// Trait for formatting tool calls and results
pub trait ToolFormatter: Send + Sync {
    /// Format tool call and result in compact mode (single line summary)
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>>;

    /// Format tool call and result in detailed mode (full parameters and output)
    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>>;
}

lazy_static! {
    static ref FORMATTERS: HashMap<&'static str, Box<dyn ToolFormatter>> = {
        use tools::tools::*;
        
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
        map.insert(crate::tools::fetch::FETCH_TOOL_NAME, Box::new(FetchFormatter));
        map.insert(crate::tools::dispatch_agent::DISPATCH_AGENT_TOOL_NAME, Box::new(DispatchAgentFormatter));
        
        map
    };
    
    static ref DEFAULT_FORMATTER: Box<dyn ToolFormatter> = Box::new(DefaultFormatter);
}

/// Get formatter for a tool name, falling back to default
pub fn get_formatter(tool_name: &str) -> &'static dyn ToolFormatter {
    FORMATTERS
        .get(tool_name)
        .map(|f| f.as_ref())
        .unwrap_or(DEFAULT_FORMATTER.as_ref())
}