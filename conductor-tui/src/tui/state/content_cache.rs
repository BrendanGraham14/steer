use once_cell::sync::Lazy;
use std::collections::{HashMap, VecDeque, hash_map::RandomState};
use std::hash::{BuildHasher, Hash, Hasher};
use tracing::debug;

use crate::tui::widgets::message_list::{MessageContent, ViewMode};

/// Global hasher builder with a fixed random seed so that hashes are stable
static HASHER_BUILDER: Lazy<RandomState> = Lazy::new(RandomState::new);

/// Quantized width to reduce cache misses on small terminal resizes
fn quantize_width(width: u16) -> u16 {
    // Round to nearest 10 to reduce cache misses on small resizes
    ((width + 5) / 10) * 10
}

/// Hash the content of a message to detect changes
fn hash_message_content(content: &MessageContent) -> u64 {
    let mut hasher = HASHER_BUILDER.build_hasher();

    match content {
        MessageContent::User { blocks, .. } => {
            "user".hash(&mut hasher);
            for block in blocks {
                match block {
                    crate::app::conversation::UserContent::Text { text } => {
                        "text".hash(&mut hasher);
                        text.hash(&mut hasher);
                    }
                    crate::app::conversation::UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => {
                        "command_exec".hash(&mut hasher);
                        command.hash(&mut hasher);
                        stdout.hash(&mut hasher);
                        stderr.hash(&mut hasher);
                        exit_code.hash(&mut hasher);
                    }
                    crate::app::conversation::UserContent::AppCommand { command, response } => {
                        "app_command".hash(&mut hasher);
                        format!("{:?}", command).hash(&mut hasher);
                        if let Some(resp) = response {
                            format!("{:?}", resp).hash(&mut hasher);
                        }
                    }
                }
            }
        }
        MessageContent::Assistant { blocks, .. } => {
            "assistant".hash(&mut hasher);
            for block in blocks {
                match block {
                    crate::app::conversation::AssistantContent::Text { text } => {
                        "text".hash(&mut hasher);
                        text.hash(&mut hasher);
                    }
                    crate::app::conversation::AssistantContent::ToolCall { tool_call } => {
                        "tool_call".hash(&mut hasher);
                        tool_call.id.hash(&mut hasher);
                        tool_call.name.hash(&mut hasher);
                        tool_call.parameters.to_string().hash(&mut hasher);
                    }
                    crate::app::conversation::AssistantContent::Thought { thought } => {
                        "thought".hash(&mut hasher);
                        thought.display_text().hash(&mut hasher);
                    }
                }
            }
        }
        MessageContent::Tool { call, result, .. } => {
            "tool".hash(&mut hasher);
            call.id.hash(&mut hasher);
            call.name.hash(&mut hasher);
            call.parameters.to_string().hash(&mut hasher);
            if let Some(result) = result {
                match result {
                    crate::app::conversation::ToolResult::Success { output } => {
                        "success".hash(&mut hasher);
                        output.hash(&mut hasher);
                    }
                    crate::app::conversation::ToolResult::Error { error } => {
                        "error".hash(&mut hasher);
                        error.hash(&mut hasher);
                    }
                }
            }
        }
    }

    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    message_id: String,
    content_hash: u64,
    view_mode: ViewMode,
    width_bucket: u16,
}

impl CacheKey {
    fn new(content: &MessageContent, view_mode: ViewMode, width: u16) -> Self {
        Self {
            message_id: content.id().to_string(),
            content_hash: hash_message_content(content),
            view_mode,
            width_bucket: quantize_width(width),
        }
    }
}

#[derive(Debug)]
pub struct ContentCache {
    cache: HashMap<CacheKey, u16>, // Just cache heights
    access_order: VecDeque<CacheKey>,
    max_size: usize,
    hits: usize,
    misses: usize,
}

impl ContentCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            access_order: VecDeque::new(),
            max_size: 10000, // Cache up to 10000 message heights
            hits: 0,
            misses: 0,
        }
    }

    /// Get cached height or calculate it fresh
    pub fn get_or_parse_height<F>(
        &mut self,
        content: &MessageContent,
        view_mode: ViewMode,
        width: u16,
        calculator: F,
    ) -> u16
    where
        F: FnOnce(&MessageContent, ViewMode, u16) -> u16,
    {
        let key = CacheKey::new(content, view_mode, width);

        // Check cache first
        if let Some(&height) = self.cache.get(&key) {
            self.hits += 1;
            self.update_access_order(&key);
            debug!(target: "content_cache", "🎯 CACHE HIT for {}", content.id());
            return height;
        }

        // Cache miss - calculate fresh
        self.misses += 1;
        debug!(target: "content_cache", "❌ CACHE MISS #{} for {} (cache size: {})", 
              self.misses, content.id(), self.cache.len());

        let height = calculator(content, view_mode, width);

        // Store in cache
        self.insert(key, height);

        height
    }

    /// Invalidate cache entries for a specific message
    pub fn invalidate_message(&mut self, message_id: &str) {
        let keys_to_remove: Vec<_> = self
            .cache
            .keys()
            .filter(|k| k.message_id == message_id)
            .cloned()
            .collect();

        for key in keys_to_remove {
            self.cache.remove(&key);
            self.access_order.retain(|k| k != &key);
        }

        debug!(target: "content_cache", "Invalidated cache for message {}", message_id);
    }

    /// Clear all cache entries (e.g., on major UI changes)
    pub fn clear(&mut self) {
        let old_size = self.cache.len();
        self.cache.clear();
        self.access_order.clear();
        debug!(target: "content_cache", "Cleared {} cache entries", old_size);
    }

    /// Get cache statistics
    pub fn stats(&self) -> (usize, usize, f64) {
        let total = self.hits + self.misses;
        let hit_rate = if total > 0 {
            self.hits as f64 / total as f64
        } else {
            0.0
        };
        (self.hits, self.misses, hit_rate)
    }

    /// Log a performance summary
    pub fn log_summary(&self) {
        let (hits, misses, hit_rate) = self.stats();
        let total = hits + misses;
        if total > 0 {
            debug!(target: "content_cache", "Cache performance: {} total requests, {} cached entries, {:.1}% hit rate", 
                  total, self.cache.len(), hit_rate * 100.0);
        }
    }

    fn insert(&mut self, key: CacheKey, value: u16) {
        // Remove if already exists to update position
        if self.cache.contains_key(&key) {
            self.access_order.retain(|k| k != &key);
        }

        self.cache.insert(key.clone(), value);
        self.access_order.push_front(key);

        // Evict LRU if over capacity
        while self.cache.len() > self.max_size {
            if let Some(lru_key) = self.access_order.pop_back() {
                self.cache.remove(&lru_key);
            }
        }
    }

    fn update_access_order(&mut self, key: &CacheKey) {
        // Move to front
        self.access_order.retain(|k| k != key);
        self.access_order.push_front(key.clone());
    }
}

impl Default for ContentCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::UserContent;
    use crate::tui::widgets::content_renderer::{ContentRenderer, DefaultContentRenderer};

    #[test]
    fn test_cache_key_equality() {
        let message = MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Hello world".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        };

        let key1 = CacheKey::new(&message, ViewMode::Compact, 80);
        let key2 = CacheKey::new(&message, ViewMode::Compact, 80);

        assert_eq!(
            key1, key2,
            "Cache keys should be equal for identical inputs"
        );
        assert_eq!(
            key1.content_hash, key2.content_hash,
            "Content hashes should match"
        );
    }

    #[test]
    fn test_cache_key_hash_consistency() {
        let message = MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Hello world".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        };

        let hash1 = hash_message_content(&message);
        let hash2 = hash_message_content(&message);

        assert_eq!(hash1, hash2, "Content hash should be consistent");
    }

    #[test]
    fn test_cache_hit_after_insert() {
        let mut cache = ContentCache::new();
        let renderer = DefaultContentRenderer;

        let message = MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Test message".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        };

        // First access - should be a miss
        let height1 =
            cache.get_or_parse_height(&message, ViewMode::Compact, 80, |msg, mode, width| {
                renderer.calculate_height(msg, mode, width)
            });

        assert_eq!(cache.hits, 0, "First access should be a miss");
        assert_eq!(cache.misses, 1, "Should have one miss");

        // Second access with EXACT same parameters - should be a hit
        let height2 =
            cache.get_or_parse_height(&message, ViewMode::Compact, 80, |msg, mode, width| {
                panic!("Calculator should not be called on cache hit!");
            });

        assert_eq!(cache.hits, 1, "Second access should be a hit");
        assert_eq!(cache.misses, 1, "Should still have only one miss");
        assert_eq!(height1, height2, "Cached height should match");
    }

    #[test]
    fn test_cache_miss_on_different_mode() {
        let mut cache = ContentCache::new();
        let renderer = DefaultContentRenderer;

        let message = MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Test message".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        };

        // First access with Compact mode
        cache.get_or_parse_height(&message, ViewMode::Compact, 80, |msg, mode, width| {
            renderer.calculate_height(msg, mode, width)
        });

        assert_eq!(cache.misses, 1);

        // Second access with Detailed mode - should be a miss
        cache.get_or_parse_height(&message, ViewMode::Detailed, 80, |msg, mode, width| {
            renderer.calculate_height(msg, mode, width)
        });

        assert_eq!(cache.misses, 2, "Different mode should cause cache miss");
        assert_eq!(cache.hits, 0, "Should have no hits");
    }

    #[test]
    fn test_cache_with_width_quantization() {
        let mut cache = ContentCache::new();
        let renderer = DefaultContentRenderer;

        let message = MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Test message".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        };

        // Access with width 78
        cache.get_or_parse_height(&message, ViewMode::Compact, 78, |msg, mode, width| {
            renderer.calculate_height(msg, mode, width)
        });

        assert_eq!(cache.misses, 1);

        // Access with width 82 - should hit cache due to quantization (both round to 80)
        cache.get_or_parse_height(&message, ViewMode::Compact, 82, |msg, mode, width| {
            panic!("Should hit cache due to width quantization!");
        });

        assert_eq!(cache.hits, 1, "Width quantization should enable cache hit");
        assert_eq!(cache.misses, 1, "Should still have only one miss");
    }

    #[test]
    fn test_cache_eviction_with_many_messages() {
        let mut cache = ContentCache::new();
        let renderer = DefaultContentRenderer;

        // Create more messages than old cache capacity (but less than new 10K)
        let mut messages = Vec::new();
        for i in 0..250 {
            messages.push(MessageContent::User {
                id: format!("test-{}", i),
                blocks: vec![UserContent::Text {
                    text: format!("Message {}", i),
                }],
                timestamp: "2023-01-01T00:00:00Z".to_string(),
            });
        }

        // First pass - cache all messages
        for msg in &messages {
            cache.get_or_parse_height(msg, ViewMode::Compact, 80, |m, mode, width| {
                renderer.calculate_height(m, mode, width)
            });
        }

        // Cache should contain all messages (250 < 10000)
        assert_eq!(cache.cache.len(), 250, "Cache should contain all messages");

        // Access first 50 messages again - they should NOT have been evicted
        let mut evicted_count = 0;
        for msg in messages.iter().take(50) {
            let stats_before = cache.stats();
            cache.get_or_parse_height(msg, ViewMode::Compact, 80, |m, mode, width| {
                renderer.calculate_height(m, mode, width)
            });
            let stats_after = cache.stats();

            if stats_after.1 > stats_before.1 {
                evicted_count += 1;
            }
        }

        // Most early messages should NOT have been evicted with larger cache
        assert!(
            evicted_count == 0,
            "Expected no messages to be evicted with larger cache, but {} were",
            evicted_count
        );

        println!(
            "Cache thrashing test: {} of 50 early messages were evicted",
            evicted_count
        );
        cache.log_summary();
    }

    #[test]
    fn test_cache_eviction_at_capacity() {
        let mut cache = ContentCache::new();
        cache.max_size = 10; // Small cache for testing
        let renderer = DefaultContentRenderer;

        // Create more messages than small cache capacity
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(MessageContent::User {
                id: format!("test-{}", i),
                blocks: vec![UserContent::Text {
                    text: format!("Message {}", i),
                }],
                timestamp: "2023-01-01T00:00:00Z".to_string(),
            });
        }

        // Cache all messages
        for msg in &messages {
            cache.get_or_parse_height(msg, ViewMode::Compact, 80, |m, mode, width| {
                renderer.calculate_height(m, mode, width)
            });
        }

        // Cache should be at small capacity
        assert_eq!(cache.cache.len(), 10, "Cache should be at capacity");

        // Early messages should have been evicted
        let stats_before = cache.stats();
        cache.get_or_parse_height(&messages[0], ViewMode::Compact, 80, |m, mode, width| {
            renderer.calculate_height(m, mode, width)
        });
        let stats_after = cache.stats();

        assert_eq!(
            stats_after.1,
            stats_before.1 + 1,
            "First message should have been evicted"
        );
    }

    #[test]
    fn test_width_quantization() {
        assert_eq!(quantize_width(75), 80);
        assert_eq!(quantize_width(80), 80);
        assert_eq!(quantize_width(85), 90);
        assert_eq!(quantize_width(83), 80);
    }

    #[test]
    fn test_no_overflow_with_large_values() {
        let mut cache = ContentCache::new();
        let renderer = DefaultContentRenderer;

        let message = MessageContent::User {
            id: "test-overflow".to_string(),
            blocks: vec![UserContent::Text {
                text: "Test message".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        };

        // Test with very large width that could cause overflow
        let height = cache.get_or_parse_height(
            &message,
            ViewMode::Compact,
            u16::MAX - 10,
            |msg, mode, width| {
                // Simulate a height calculation that won't overflow
                10u16
            },
        );

        assert_eq!(height, 10, "Height should be calculated without overflow");
    }
}
