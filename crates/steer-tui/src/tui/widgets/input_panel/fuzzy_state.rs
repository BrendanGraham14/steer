//! Fuzzy finder helper functions for the input panel

/// Helper struct for fuzzy finder state operations on text content
#[derive(Debug)]
pub struct FuzzyFinderHelper;

impl FuzzyFinderHelper {
    /// Get cursor byte offset for a textarea
    pub fn get_cursor_byte_offset(content: &str, cursor_row: usize, cursor_col: usize) -> usize {
        let lines: Vec<&str> = content.split('\n').collect();
        let mut offset = 0;

        // Add up all the bytes from lines before the cursor.
        // `cursor_col` is a character index, not a byte index, so we must
        // translate columns to UTF-8 byte offsets explicitly.
        for (i, line) in lines.iter().enumerate() {
            match i.cmp(&cursor_row) {
                std::cmp::Ordering::Less => {
                    offset += line.len() + 1; // +1 for the newline
                }
                std::cmp::Ordering::Equal => {
                    offset += line
                        .chars()
                        .take(cursor_col)
                        .map(char::len_utf8)
                        .sum::<usize>();
                    break;
                }
                std::cmp::Ordering::Greater => break,
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

        // Cursor must be at or after the trigger character ("@" or "/")
        if cursor_offset < at_pos {
            return false;
        }

        // Cursor exactly on the trigger is allowed (user just jumped there)
        if cursor_offset == at_pos {
            return true;
        }

        // Cursor after trigger: ensure no whitespace in-between
        content[at_pos + 1..cursor_offset]
            .chars()
            .all(|c| !c.is_whitespace())
    }

    /// Extract the current fuzzy query from content
    pub fn get_current_fuzzy_query(
        trigger_position: usize,
        cursor_offset: usize,
        content: &str,
    ) -> Option<String> {
        if cursor_offset > trigger_position + 1 {
            let query_text = &content[trigger_position + 1..cursor_offset];
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
        assert!(FuzzyFinderHelper::is_in_fuzzy_query(Some(6), 15, content));

        // Cursor before trigger
        assert!(!FuzzyFinderHelper::is_in_fuzzy_query(Some(6), 5, content));

        // No trigger position
        assert!(!FuzzyFinderHelper::is_in_fuzzy_query(None, 10, content));

        // Whitespace in query
        let content_with_space = "Check @ file.txt";
        assert!(!FuzzyFinderHelper::is_in_fuzzy_query(
            Some(6),
            12,
            content_with_space
        ));

        // Cursor exactly at trigger
        assert!(FuzzyFinderHelper::is_in_fuzzy_query(Some(6), 6, content));
    }

    #[test]
    fn test_get_current_fuzzy_query() {
        let content = "Check @src/main.rs here";

        // Trigger position is index of '@'
        let trigger = 6; // 'C=0 h=1 e=2 c=3 k=4 space=5 @=6'

        // Valid query
        assert_eq!(
            FuzzyFinderHelper::get_current_fuzzy_query(trigger, 18, content),
            Some("src/main.rs".to_string())
        );

        // Partial query
        assert_eq!(
            FuzzyFinderHelper::get_current_fuzzy_query(trigger, 10, content),
            Some("src".to_string())
        );

        // No query (cursor at trigger)
        assert_eq!(
            FuzzyFinderHelper::get_current_fuzzy_query(trigger, trigger, content),
            None
        );
    }
}
