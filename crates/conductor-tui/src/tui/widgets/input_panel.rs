use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use tui_textarea::TextArea;

use conductor_tools::schema::ToolCall;

use crate::tui::InputMode;
use crate::tui::get_spinner_char;

/// Render the input/approval panel at the bottom of the screen.
#[allow(clippy::too_many_arguments)]
pub fn render_input_panel(
    f: &mut Frame,
    area: Rect,
    textarea: &TextArea,
    input_mode: InputMode,
    current_approval: Option<&ToolCall>,
    is_processing: bool,
    spinner_state: usize,
    edit_selection_messages: &[(String, String)],
    edit_selection_index: usize,
) -> Result<()> {
    // Render approval prompt if needed
    if let Some(tool_call) = current_approval {
        let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
        let preview_lines = formatter.compact(
            &tool_call.parameters,
            &None,
            (area.width.saturating_sub(4)) as usize,
        );

        let mut approval_text = vec![
            Line::from(vec![
                Span::styled("Tool '", Style::default().fg(Color::White)),
                Span::styled(
                    &tool_call.name,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("' requests approval:", Style::default().fg(Color::White)),
            ]),
            Line::from(""),
        ];
        approval_text.extend(preview_lines);

        let title = Line::from(vec![
            Span::raw(" Tool Approval Required "),
            Span::raw("─ "),
            Span::styled(
                "[Y]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" once "),
            Span::styled(
                "[A]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("lways "),
            Span::styled(
                "[N]",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw("o "),
        ]);

        let approval_block = Paragraph::new(approval_text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().fg(Color::Yellow)),
        );

        f.render_widget(approval_block, area);
        return Ok(());
    }

    // Normal input / edit selection rendering
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            "{}{}",
            if is_processing {
                format!(" {}", get_spinner_char(spinner_state))
            } else {
                String::new()
            },
            match input_mode {
                InputMode::Insert => " Insert (Alt-Enter to send, Esc to cancel) ",
                InputMode::Normal =>
                    " (i to insert, ! for bash, u/d/j/k to scroll, e to edit previous messages) ",
                InputMode::BashCommand => " Bash (Enter to execute, Esc to cancel) ",
                InputMode::AwaitingApproval => " Awaiting Approval ",
                InputMode::SelectingModel => " Model Selection ",
                InputMode::ConfirmExit =>
                    " Really quit? (y/Y to confirm, any other key to cancel) ",
                InputMode::EditMessageSelection => {
                    " Select message to edit (↑↓ to navigate, Enter to select, Esc to cancel) "
                }
            }
        ))
        .style(match input_mode {
            InputMode::Insert => Style::default().fg(Color::Green),
            InputMode::Normal => Style::default().fg(Color::DarkGray),
            InputMode::BashCommand => Style::default().fg(Color::Cyan),
            InputMode::ConfirmExit => Style::default().fg(Color::Red),
            InputMode::EditMessageSelection => Style::default().fg(Color::Yellow),
            _ => Style::default(),
        });

    if input_mode == InputMode::EditMessageSelection {
        // Selection list
        let mut items: Vec<ListItem> = Vec::new();
        if edit_selection_messages.is_empty() {
            items.push(
                ListItem::new("No user messages to edit")
                    .style(Style::default().fg(Color::DarkGray)),
            );
        } else {
            let max_visible = 3;
            let total = edit_selection_messages.len();
            let (start_idx, end_idx) = if total <= max_visible {
                (0, total)
            } else {
                let half_window = max_visible / 2;
                if edit_selection_index < half_window {
                    (0, max_visible)
                } else if edit_selection_index >= total - half_window {
                    (total - max_visible, total)
                } else {
                    let start = edit_selection_index - half_window;
                    (start, start + max_visible)
                }
            };

            for idx in start_idx..end_idx {
                let (_, content) = &edit_selection_messages[idx];
                let preview = content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(area.width.saturating_sub(4) as usize)
                    .collect::<String>();
                items.push(ListItem::new(preview));
            }

            let mut list_state = ListState::default();
            list_state.select(Some(edit_selection_index.saturating_sub(start_idx)));

            let highlight_style = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED);

            let list = List::new(items)
                .block(input_block)
                .highlight_style(highlight_style);
            f.render_stateful_widget(list, area, &mut list_state);
            return Ok(());
        }

        // Empty list fallback
        let list = List::new(items).block(input_block);
        f.render_widget(list, area);
        return Ok(());
    }

    // Default: textarea
    let mut textarea_with_block = textarea.clone();
    textarea_with_block.set_block(input_block);
    f.render_widget(&textarea_with_block, area);

    // Scrollbar when needed
    let textarea_height = area.height.saturating_sub(2);
    let content_lines = textarea.lines().len();
    if content_lines > textarea_height as usize {
        let (cursor_row, _) = textarea.cursor();
        let mut scrollbar_state = ScrollbarState::new(content_lines)
            .position(cursor_row)
            .viewport_content_length(textarea_height as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .thumb_style(Style::default().fg(Color::Gray));
        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height - 2,
        };
        f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    Ok(())
}
