use ratatui::crossterm::event::KeyCode;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct DoubleTapTracker {
    last_key_times: HashMap<KeyCode, Instant>,
}

impl Default for DoubleTapTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DoubleTapTracker {
    pub fn new() -> Self {
        Self {
            last_key_times: HashMap::new(),
        }
    }

    /// Record a key press
    pub fn record_key(&mut self, key: KeyCode) {
        self.last_key_times.insert(key, Instant::now());
    }

    /// Check if a key press would be a double-tap
    pub fn is_double_tap(&self, key: KeyCode, threshold: Duration) -> bool {
        if let Some(&last_time) = self.last_key_times.get(&key) {
            Instant::now().duration_since(last_time) <= threshold
        } else {
            false
        }
    }

    /// Clear tracking for a specific key
    pub fn clear_key(&mut self, key: &KeyCode) {
        self.last_key_times.remove(key);
    }

    /// Clear all tracked keys
    pub fn clear_all(&mut self) {
        self.last_key_times.clear();
    }

    /// Clean up entries older than the given duration
    pub fn cleanup(&mut self, older_than: Duration) {
        let now = Instant::now();
        self.last_key_times
            .retain(|_, &mut time| now.duration_since(time) <= older_than);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_tap_detection() {
        let mut tracker = DoubleTapTracker::new();

        // First tap - not a double tap
        assert!(!tracker.is_double_tap(KeyCode::Esc, Duration::from_millis(300)));
        tracker.record_key(KeyCode::Esc);

        // Quick second tap - is a double tap
        assert!(tracker.is_double_tap(KeyCode::Esc, Duration::from_millis(300)));
        tracker.clear_key(&KeyCode::Esc);

        // Third tap - not a double tap (cleared)
        assert!(!tracker.is_double_tap(KeyCode::Esc, Duration::from_millis(300)));

        // Test different keys independently
        tracker.record_key(KeyCode::Char('a'));
        assert!(tracker.is_double_tap(KeyCode::Char('a'), Duration::from_millis(300)));

        // Test timeout
        tracker.record_key(KeyCode::Enter);
        std::thread::sleep(Duration::from_millis(400));
        assert!(!tracker.is_double_tap(KeyCode::Enter, Duration::from_millis(300)));
    }

    #[test]
    fn test_cleanup() {
        let mut tracker = DoubleTapTracker::new();

        // Add some keys
        tracker.record_key(KeyCode::Char('a'));
        tracker.record_key(KeyCode::Char('b'));

        // Wait for them to expire
        std::thread::sleep(Duration::from_millis(150));

        // Cleanup should remove old entries
        tracker.cleanup(Duration::from_millis(100));

        // These should not be double-taps anymore
        assert!(!tracker.is_double_tap(KeyCode::Char('a'), Duration::from_millis(100)));
        assert!(!tracker.is_double_tap(KeyCode::Char('b'), Duration::from_millis(100)));
    }
}
