use super::{ToolFormatter, helpers::*};
use crate::tui::theme::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use steer_core::app::conversation::ToolResult;
use steer_core::tools::{DispatchAgentParams, WorkspaceTarget};

pub struct DispatchAgentFormatter;

impl ToolFormatter for DispatchAgentFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<DispatchAgentParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid agent params",
                theme.error_text(),
            ))];
        };

        let preview = if params.prompt.len() > 60 {
            format!("{}...", &params.prompt[..57])
        } else {
            params.prompt.clone()
        };

        let workspace_summary = match &params.workspace {
            WorkspaceTarget::Current => "current".to_string(),
            WorkspaceTarget::New { name } => format!("{name} (new)"),
        };

        let info = match result {
            Some(ToolResult::Agent(agent_result)) => {
                let line_count = agent_result.content.lines().count();
                format!("{line_count} lines")
            }
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "running...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("workspace={workspace_summary} "), theme.subtle_text()),
            Span::styled(format!("task='{preview}' "), Style::default()),
            Span::styled(format!("({info})"), theme.subtle_text()),
        ]));

        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<DispatchAgentParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid agent params",
                theme.error_text(),
            ))];
        };

        let workspace_label = match &params.workspace {
            WorkspaceTarget::Current => "current".to_string(),
            WorkspaceTarget::New { name } => format!("{name} (new)"),
        };

        lines.push(Line::from(vec![
            Span::styled("Workspace: ", theme.subtle_text()),
            Span::styled(workspace_label, Style::default()),
        ]));

        lines.push(Line::from(Span::styled("Instructions:", theme.text())));
        for line in params.prompt.lines() {
            for wrapped_line in textwrap::wrap(line, wrap_width) {
                lines.push(Line::from(Span::styled(
                    wrapped_line.to_string(),
                    Style::default(),
                )));
            }
        }

        // Show output if we have results
        if let Some(result) = result {
            match result {
                ToolResult::Agent(agent_result) => {
                    if !agent_result.content.trim().is_empty() {
                        lines.push(separator_line(wrap_width, theme.dim_text()));

                        const MAX_OUTPUT_LINES: usize = 30;
                        let (output_lines, truncated) =
                            truncate_lines(&agent_result.content, MAX_OUTPUT_LINES);

                        for line in output_lines {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more lines)",
                                    agent_result.content.lines().count() - MAX_OUTPUT_LINES
                                ),
                                theme.subtle_text(),
                            )));
                        }
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(separator_line(wrap_width, theme.dim_text()));
                    lines.push(Line::from(Span::styled(
                        tool_error_user_message(error).into_owned(),
                        theme.error_text(),
                    )));
                }
                _ => {
                    lines.push(Line::from(Span::styled(
                        "Unexpected result type",
                        theme.error_text(),
                    )));
                }
            }
        }

        lines
    }

    fn approval(&self, params: &Value, wrap_width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<DispatchAgentParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid agent params",
                theme.error_text(),
            ))];
        };

        let workspace_label = match &params.workspace {
            WorkspaceTarget::Current => "current".to_string(),
            WorkspaceTarget::New { name } => format!("{name} (new)"),
        };

        lines.push(Line::from(vec![
            Span::styled("Workspace: ", theme.subtle_text()),
            Span::styled(workspace_label, Style::default()),
        ]));

        lines.push(Line::from(Span::styled(
            "Instructions:",
            theme.subtle_text(),
        )));
        for line in params.prompt.lines() {
            for wrapped_line in textwrap::wrap(line, wrap_width) {
                lines.push(Line::from(Span::styled(
                    wrapped_line.to_string(),
                    Style::default(),
                )));
            }
        }

        lines
    }
}
