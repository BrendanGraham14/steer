use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cache for workspace files to enable fast fuzzy searching
#[derive(Clone, Debug)]
pub struct FileCache {
    /// Cached file paths from the workspace
    files: Arc<RwLock<Vec<String>>>,
    /// Session ID this cache belongs to
    session_id: String,
}

impl FileCache {
    /// Create a new empty file cache
    pub fn new(session_id: String) -> Self {
        Self {
            files: Arc::new(RwLock::new(Vec::new())),
            session_id,
        }
    }

    /// Update the cache with new file paths
    pub async fn update(&self, files: Vec<String>) {
        let mut cache = self.files.write().await;
        *cache = files;
    }

    /// Clear the cache
    pub async fn clear(&self) {
        let mut cache = self.files.write().await;
        cache.clear();
    }

    /// Check if the cache is empty
    pub async fn is_empty(&self) -> bool {
        let cache = self.files.read().await;
        cache.is_empty()
    }

    /// Get the number of files in the cache
    pub async fn len(&self) -> usize {
        let cache = self.files.read().await;
        cache.len()
    }

    /// Search files with fuzzy matching
    pub async fn fuzzy_search(&self, query: &str, max_results: Option<usize>) -> Vec<String> {
        if query.is_empty() {
            // If no query, return all files up to limit
            let cache = self.files.read().await;
            let max = max_results.unwrap_or(cache.len());
            return cache.iter().take(max).cloned().collect();
        }

        let cache = self.files.read().await;
        let matcher = SkimMatcherV2::default();

        let mut scored_files: Vec<(i64, String)> = cache
            .iter()
            .filter_map(|file| {
                matcher
                    .fuzzy_match(file, query)
                    .map(|score| (score, file.clone()))
            })
            .collect();

        // Sort by score (highest first)
        scored_files.sort_by(|a, b| b.0.cmp(&a.0));

        // Apply limit if specified

        if let Some(max) = max_results {
            scored_files
                .into_iter()
                .take(max)
                .map(|(_, file)| file)
                .collect()
        } else {
            scored_files.into_iter().map(|(_, file)| file).collect()
        }
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

#[cfg(test)]
mod tests {
    use super::FileCache;

    #[tokio::test]
    async fn test_fuzzy_search_matches_query() {
        let cache = FileCache::new("session-1".to_string());
        cache
            .update(vec![
                "src/main.rs".to_string(),
                "src/lib.rs".to_string(),
                "README.md".to_string(),
            ])
            .await;

        let results = cache.fuzzy_search("main", None).await;
        assert!(results.iter().any(|path| path == "src/main.rs"));
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_fuzzy_search_empty_query_limit() {
        let cache = FileCache::new("session-2".to_string());
        cache
            .update(vec![
                "src/main.rs".to_string(),
                "src/lib.rs".to_string(),
                "README.md".to_string(),
            ])
            .await;

        let results = cache.fuzzy_search("", Some(2)).await;
        assert_eq!(results.len(), 2);
    }
}
