//! Text area widget with scrollbar support

use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, StatefulWidget, Widget};
use ratatui::widgets::{Block, Scrollbar, ScrollbarOrientation, ScrollbarState};
use tui_textarea::TextArea;

use crate::tui::InputMode;
use crate::tui::theme::{Component, Theme};

/// Widget wrapper for TextArea with scrollbar support
#[derive(Debug)]
pub struct TextAreaWidget<'a> {
    textarea: &'a mut TextArea<'static>,
    theme: &'a Theme,
    block: Option<Block<'a>>,
    mode: Option<InputMode>,
    is_editing: bool,
}

impl<'a> TextAreaWidget<'a> {
    /// Create a new text area widget
    pub fn new(textarea: &'a mut TextArea<'static>, theme: &'a Theme) -> Self {
        Self {
            textarea,
            theme,
            block: None,
            mode: None,
            is_editing: false,
        }
    }

    /// Set the block for the text area
    pub fn with_block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Set the input mode for styling
    pub fn with_mode(mut self, mode: InputMode) -> Self {
        self.mode = Some(mode);
        self
    }

    pub fn with_editing(mut self, is_editing: bool) -> Self {
        self.is_editing = is_editing;
        self
    }
}

impl<'a> Widget for TextAreaWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Take ownership of block to avoid borrow issues
        let (inner_area, theme, _mode) = if let Some(block) = self.block {
            let styled_block = if let Some(mode) = self.mode {
                apply_mode_styling(block, mode, self.theme, self.is_editing)
            } else if self.is_editing {
                apply_mode_styling(block, InputMode::Simple, self.theme, true)
            } else {
                block
            };
            let inner = styled_block.inner(area);
            styled_block.render(area, buf);
            (inner, self.theme, self.mode)
        } else {
            (area, self.theme, self.mode)
        };

        // Calculate if we need scrollbar before rendering textarea
        let textarea_height = inner_area.height;
        let content_lines = self.textarea.lines().len();
        let needs_scrollbar = content_lines > textarea_height as usize;
        let cursor_row = self.textarea.cursor().0;

        // Render the text area without its own block
        self.textarea.set_block(Block::default());
        self.textarea.render(inner_area, buf);

        // Render scrollbar if needed
        if needs_scrollbar {
            let mut scrollbar_state = ScrollbarState::new(content_lines)
                .position(cursor_row)
                .viewport_content_length(textarea_height as usize);

            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .thumb_style(theme.style(Component::DimText));

            let scrollbar_area = Rect {
                x: area.x + area.width - 1,
                y: area.y + 1,
                width: 1,
                height: area.height - 2,
            };

            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

/// Apply mode-specific styling to a block
fn apply_mode_styling<'a>(
    mut block: Block<'a>,
    mode: InputMode,
    theme: &Theme,
    is_editing: bool,
) -> Block<'a> {
    if is_editing {
        let style = theme.style(Component::InputPanelBorderEdit);
        return block.style(style).border_style(style);
    }

    match mode {
        InputMode::Simple | InputMode::VimInsert => {
            // Active border and text style
            let active = theme.style(Component::InputPanelBorderActive);
            block = block.style(active).border_style(active);
        }
        InputMode::VimNormal => {
            // Keep text style the same as VimInsert (active) but dim the border
            let text_style = theme.style(Component::InputPanelBorderActive);
            let border_dim = theme.style(Component::InputPanelBorder);
            block = block.style(text_style).border_style(border_dim);
        }
        InputMode::BashCommand => {
            let style = theme.style(Component::InputPanelBorderCommand);
            block = block.style(style).border_style(style);
        }
        InputMode::ConfirmExit => {
            let style = theme.style(Component::InputPanelBorderError);
            block = block.style(style).border_style(style);
        }
        InputMode::EditMessageSelection => {
            let style = theme.style(Component::InputPanelBorderCommand);
            block = block.style(style).border_style(style);
        }
        InputMode::FuzzyFinder => {
            let style = theme.style(Component::InputPanelBorderActive);
            block = block.style(style).border_style(style);
        }
        _ => {}
    }
    block
}
