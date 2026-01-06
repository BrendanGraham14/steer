pub mod astgrep;
pub mod bash;
pub mod dispatch_agent;
pub mod edit;
pub mod fetch;
pub mod glob;
pub mod grep;
pub mod ls;
pub mod replace;
pub mod todo;
pub mod view;

pub use astgrep::AstGrepTool;
pub use bash::BashTool;
pub use dispatch_agent::DispatchAgentTool;
pub use edit::{EditTool, MultiEditTool};
pub use fetch::FetchTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use replace::ReplaceTool;
pub use todo::{TodoReadTool, TodoWriteTool};
pub use view::ViewTool;

use steer_tools::ExecutionContext as ToolsExecutionContext;

use super::static_tool::StaticToolContext;

pub(crate) fn to_tools_context(ctx: &StaticToolContext) -> ToolsExecutionContext {
    ToolsExecutionContext::new(ctx.tool_call_id.0.clone())
        .with_cancellation_token(ctx.cancellation_token.clone())
        .with_working_directory(ctx.services.workspace.working_directory().to_path_buf())
}
