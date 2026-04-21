use super::{
    ToolFormatter,
    helpers::{separator_line, tool_error_user_message},
};
use crate::tui::theme::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use steer_grpc::client_api::ToolResult;
use steer_tools::tools::wc::WcParams;

pub struct WcFormatter;

impl ToolFormatter for WcFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<WcParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid wc params",
                theme.error_text(),
            ))];
        };

        let info = match result {
            Some(ToolResult::Wc(w)) => format!("{}L {}W {}B", w.lines, w.words, w.bytes),
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "counting...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("file='{}' ", params.file_path), Style::default()),
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

        let Ok(params) = serde_json::from_value::<WcParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid wc params",
                theme.error_text(),
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("File: {}", params.file_path),
            Style::default(),
        )));

        if let Some(result) = result {
            match result {
                ToolResult::Wc(w) => {
                    lines.push(separator_line(wrap_width, theme.dim_text()));
                    lines.push(Line::from(Span::raw(format!(
                        "lines: {}\nwords: {}\nbytes: {}",
                        w.lines, w.words, w.bytes
                    ))));
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
}
