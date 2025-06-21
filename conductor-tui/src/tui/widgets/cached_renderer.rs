use ratatui::{buffer::Buffer, layout::Rect};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::debug;

use super::content_renderer::{ContentRenderer, DefaultContentRenderer};
use crate::tui::state::content_cache::ContentCache;
use crate::tui::widgets::message_list::{MessageContent, ViewMode};

// Key for caching rendered content
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RenderCacheKey {
    message_id: String,
    content_hash: u64,
    view_mode: ViewMode,
    area: Rect,
}

impl RenderCacheKey {
    fn new(content: &MessageContent, view_mode: ViewMode, area: Rect) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        // Hash the content to detect changes
        match content {
            MessageContent::User { blocks, .. } => {
                "user".hash(&mut hasher);
                for block in blocks {
                    format!("{:?}", block).hash(&mut hasher);
                }
            }
            MessageContent::Assistant { blocks, .. } => {
                "assistant".hash(&mut hasher);
                for block in blocks {
                    format!("{:?}", block).hash(&mut hasher);
                }
            }
            MessageContent::Tool { call, result, .. } => {
                "tool".hash(&mut hasher);
                call.id.hash(&mut hasher);
                call.name.hash(&mut hasher);
                call.parameters.to_string().hash(&mut hasher);
                if let Some(r) = result {
                    format!("{:?}", r).hash(&mut hasher);
                }
            }
        }

        Self {
            message_id: content.id().to_string(),
            content_hash: hasher.finish(),
            view_mode,
            area,
        }
    }
}

// Cached render data - stores the cells that were rendered
#[derive(Clone)]
struct CachedRender {
    cells: Vec<ratatui::buffer::Cell>,
}

#[derive(Default)]
struct CacheStats {
    render_hits: usize,
    render_misses: usize,
}

/// A ContentRenderer wrapper that caches height calculations and rendered content
pub struct CachedContentRenderer {
    inner: DefaultContentRenderer,
    height_cache: Arc<RwLock<ContentCache>>,
    render_cache: Arc<RwLock<HashMap<RenderCacheKey, CachedRender>>>,
    max_render_cache_size: usize,
    stats: Arc<RwLock<CacheStats>>,
}

impl CachedContentRenderer {
    pub fn new(cache: Arc<RwLock<ContentCache>>) -> Self {
        Self {
            inner: DefaultContentRenderer,
            height_cache: cache,
            render_cache: Arc::new(RwLock::new(HashMap::new())),
            max_render_cache_size: 1000, // Cache up to 1000 rendered messages
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    fn evict_if_needed(&self) {
        if let Ok(mut cache) = self.render_cache.write() {
            // Simple LRU-ish eviction: remove random entries if over capacity
            if cache.len() > self.max_render_cache_size {
                let to_remove = cache.len() - self.max_render_cache_size;
                let keys: Vec<_> = cache.keys().take(to_remove).cloned().collect();
                for key in keys {
                    cache.remove(&key);
                }
                debug!(target: "render_cache", "Evicted {} entries from render cache", to_remove);
            }
        }
    }

    pub fn log_stats(&self) {
        if let Ok(stats) = self.stats.read() {
            let total = stats.render_hits + stats.render_misses;
            if total > 0 {
                let hit_rate = stats.render_hits as f64 / total as f64;
                debug!(
                    target: "render_cache",
                    "Render cache stats: {} hits, {} misses, {:.1}% hit rate",
                    stats.render_hits,
                    stats.render_misses,
                    hit_rate * 100.0
                );
            }
        }
    }
}

impl ContentRenderer for CachedContentRenderer {
    fn render(&self, content: &MessageContent, mode: ViewMode, area: Rect, buf: &mut Buffer) {
        let cache_key = RenderCacheKey::new(content, mode, area);

        // Try to get from cache first
        if let Ok(cache) = self.render_cache.read() {
            if let Some(cached) = cache.get(&cache_key) {
                // Cache hit - update stats
                if let Ok(mut stats) = self.stats.write() {
                    stats.render_hits += 1;
                }
                debug!(target: "render_cache", "🎯 RENDER CACHE HIT for message {}", content.id());

                // Copy cached cells to the buffer
                let mut idx = 0;
                for y in area.top()..area.bottom() {
                    for x in area.left()..area.right() {
                        if idx < cached.cells.len() {
                            buf[(x, y)] = cached.cells[idx].clone();
                            idx += 1;
                        }
                    }
                }
                return;
            }
        }

        // Cache miss - update stats
        if let Ok(mut stats) = self.stats.write() {
            stats.render_misses += 1;
        }
        debug!(target: "render_cache", "❌ RENDER CACHE MISS for message {} (area: {:?})", content.id(), area);

        // Not in cache - render normally
        // First, capture the current state of the area
        let mut cells_before = Vec::with_capacity((area.width * area.height) as usize);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                cells_before.push(buf[(x, y)].clone());
            }
        }

        // Render using the inner renderer
        self.inner.render(content, mode, area, buf);

        // Capture the rendered cells
        let mut rendered_cells = Vec::with_capacity((area.width * area.height) as usize);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                rendered_cells.push(buf[(x, y)].clone());
            }
        }

        // Store in cache
        if let Ok(mut cache) = self.render_cache.write() {
            cache.insert(
                cache_key,
                CachedRender {
                    cells: rendered_cells,
                },
            );
            debug!(target: "render_cache", "Cached render for message {} (cache size: {})", content.id(), cache.len());
        }

        // Evict old entries if needed
        self.evict_if_needed();
    }

    fn calculate_height(&self, content: &MessageContent, mode: ViewMode, width: u16) -> u16 {
        // Use the height cache for height calculations
        if let Ok(mut cache) = self.height_cache.write() {
            cache.get_or_parse_height(content, mode, width, |msg, view_mode, w| {
                self.inner.calculate_height(msg, view_mode, w)
            })
        } else {
            // Fallback if mutex is poisoned
            self.inner.calculate_height(content, mode, width)
        }
    }
}
