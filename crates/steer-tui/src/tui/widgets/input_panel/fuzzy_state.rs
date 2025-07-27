//! Fuzzy finder helper functions for the input panel

/// Helper struct for fuzzy finder state operations on text content
#[derive(Debug)]
pub struct FuzzyFinderHelper;

impl FuzzyFinderHelper {
    /// Get cursor byte offset for a textarea
    pub fn get_cursor_byte_offset(content: &str, cursor_row: usize, cursor_col: usize) -> usize {
        let lines: Vec<&str> = content.split('\n').collect();
        let mut offset = 0;

        // Add up all the bytes from lines before the cursor
        for (i, line) in lines.iter().enumerate() {
            if i < cursor_row {
                offset += line.len() + 1; // +1 for the newline
            } else if i == cursor_row {
                // For the cursor line, only count up to the cursor column
                offset += line[..cursor_col.min(line.len())].len();
                break;
            }
        }

        offset
    }

    /// Check if cursor is in a valid fuzzy query position
    pub fn is_in_fuzzy_query(
        trigger_position: Option<usize>,
        cursor_offset: usize,
        content: &str,
    ) -> bool {
        let Some(at_pos) = trigger_position else {
            return false;
        };

        // Cursor must be after '@' or '/'
        if cursor_offset <= at_pos {
            return false;
        }

        // Check if there's whitespace between trigger and cursor
        content[at_pos..cursor_offset]
            .chars()
            .all(|c| !c.is_whitespace())
    }

    /// Extract the current fuzzy query from content
    pub fn get_current_fuzzy_query(
        trigger_position: usize,
        cursor_offset: usize,
        content: &str,
    ) -> Option<String> {
        if cursor_offset > trigger_position {
            let query_text = &content[trigger_position..cursor_offset];
            if !query_text.is_empty() {
                return Some(query_text.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_cursor_byte_offset() {
        let content = "Hello\nWorld\n!";

        // Beginning of first line
        assert_eq!(FuzzyFinderHelper::get_cursor_byte_offset(content, 0, 0), 0);

        // Middle of first line
        assert_eq!(FuzzyFinderHelper::get_cursor_byte_offset(content, 0, 3), 3);

        // Beginning of second line
        assert_eq!(FuzzyFinderHelper::get_cursor_byte_offset(content, 1, 0), 6);

        // End of second line
        assert_eq!(FuzzyFinderHelper::get_cursor_byte_offset(content, 1, 5), 11);

        // Third line
        assert_eq!(FuzzyFinderHelper::get_cursor_byte_offset(content, 2, 1), 13);
    }

    #[test]
    fn test_is_in_fuzzy_query() {
        let content = "Check @file.txt here";

        // Valid query position
        assert!(FuzzyFinderHelper::is_in_fuzzy_query(Some(7), 15, content));

        // Cursor before trigger
        assert!(!FuzzyFinderHelper::is_in_fuzzy_query(Some(7), 5, content));

        // No trigger position
        assert!(!FuzzyFinderHelper::is_in_fuzzy_query(None, 10, content));

        // Whitespace in query
        let content_with_space = "Check @ file.txt";
        assert!(!FuzzyFinderHelper::is_in_fuzzy_query(
            Some(7),
            12,
            content_with_space
        ));
    }

    #[test]
    fn test_get_current_fuzzy_query() {
        let content = "Check @src/main.rs here";

        // Valid query
        assert_eq!(
            FuzzyFinderHelper::get_current_fuzzy_query(7, 18, content),
            Some("src/main.rs".to_string())
        );

        // Partial query
        assert_eq!(
            FuzzyFinderHelper::get_current_fuzzy_query(7, 10, content),
            Some("src".to_string())
        );

        // No query (cursor at trigger)
        assert_eq!(
            FuzzyFinderHelper::get_current_fuzzy_query(7, 7, content),
            None
        );
    }
}
