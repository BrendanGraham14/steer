use ratatui::{buffer::Buffer, layout::Rect};
use std::sync::{Arc, Mutex};

use crate::tui::widgets::message_list::{MessageContent, ViewMode};
use crate::tui::state::content_cache::ContentCache;
use super::content_renderer::{ContentRenderer, DefaultContentRenderer};

/// A ContentRenderer wrapper that caches height calculations and parsed content
/// but always uses the original rendering logic for drawing
pub struct CachedContentRenderer {
    inner: DefaultContentRenderer,
    cache: Arc<Mutex<ContentCache>>,
}

impl CachedContentRenderer {
    pub fn new(cache: Arc<Mutex<ContentCache>>) -> Self {
        Self {
            inner: DefaultContentRenderer,
            cache,
        }
    }
}

impl ContentRenderer for CachedContentRenderer {
    fn render(&self, content: &MessageContent, mode: ViewMode, area: Rect, buf: &mut Buffer) {
        // Always use the original renderer for actual rendering
        // The cache is only used for height calculations
        self.inner.render(content, mode, area, buf);
    }

    fn calculate_height(&self, content: &MessageContent, mode: ViewMode, width: u16) -> u16 {
        // Only cache height calculations
        if let Ok(mut cache) = self.cache.lock() {
            cache.get_or_parse_height(
                content,
                mode,
                width,
                |msg, view_mode, w| {
                    self.inner.calculate_height(msg, view_mode, w)
                },
            )
        } else {
            // Fallback if mutex is poisoned
            self.inner.calculate_height(content, mode, width)
        }
    }
}