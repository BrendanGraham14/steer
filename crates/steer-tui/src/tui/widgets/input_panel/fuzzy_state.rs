//! Fuzzy finder helper functions for the input panel

/// Helper struct for fuzzy finder state operations on text content
#[derive(Debug)]
pub struct FuzzyFinderState;

impl FuzzyFinderState {
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

    /// Complete fuzzy finder by replacing the query text with the selected item
    pub fn complete_fuzzy_finder(
        content: &str,
        trigger_position: usize,
        cursor_offset: usize,
        selected_item: &str,
    ) -> (String, usize, usize) {
        let before_trigger = if trigger_position > 0 {
            &content[..trigger_position - 1]
        } else {
            ""
        };
        let after_cursor = &content[cursor_offset..];

        let new_content = format!("{before_trigger}{selected_item}{after_cursor}");

        // Calculate new cursor position
        let new_cursor_byte_pos = before_trigger.len() + selected_item.len();
        let new_cursor_row = new_content[..new_cursor_byte_pos].matches('\n').count();
        let last_newline_pos = new_content[..new_cursor_byte_pos]
            .rfind('\n')
            .map(|pos| pos + 1)
            .unwrap_or(0);
        let new_cursor_col = new_content[last_newline_pos..new_cursor_byte_pos]
            .chars()
            .count();

        (new_content, new_cursor_row, new_cursor_col)
    }

    /// Complete command fuzzy finder by replacing the query text
    pub fn complete_command_fuzzy(
        content: &str,
        _trigger_position: usize,
        cursor_offset: usize,
        selected_command: &str,
    ) -> (String, usize, usize) {
        // For commands, we want to replace everything from the start
        let after_cursor = &content[cursor_offset..];
        let new_content = format!("{selected_command}{after_cursor}");

        // Position cursor at the end of the command
        let new_cursor_row = 0; // Commands are single line
        let new_cursor_col = selected_command.chars().count();

        (new_content, new_cursor_row, new_cursor_col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_cursor_byte_offset() {
        let content = "Hello\nWorld\n!";

        // Beginning of first line
        assert_eq!(FuzzyFinderState::get_cursor_byte_offset(content, 0, 0), 0);

        // Middle of first line
        assert_eq!(FuzzyFinderState::get_cursor_byte_offset(content, 0, 3), 3);

        // Beginning of second line
        assert_eq!(FuzzyFinderState::get_cursor_byte_offset(content, 1, 0), 6);

        // End of second line
        assert_eq!(FuzzyFinderState::get_cursor_byte_offset(content, 1, 5), 11);

        // Third line
        assert_eq!(FuzzyFinderState::get_cursor_byte_offset(content, 2, 1), 13);
    }

    #[test]
    fn test_is_in_fuzzy_query() {
        let content = "Check @file.txt here";

        // Valid query position
        assert!(FuzzyFinderState::is_in_fuzzy_query(Some(7), 15, content));

        // Cursor before trigger
        assert!(!FuzzyFinderState::is_in_fuzzy_query(Some(7), 5, content));

        // No trigger position
        assert!(!FuzzyFinderState::is_in_fuzzy_query(None, 10, content));

        // Whitespace in query
        let content_with_space = "Check @ file.txt";
        assert!(!FuzzyFinderState::is_in_fuzzy_query(
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
            FuzzyFinderState::get_current_fuzzy_query(7, 18, content),
            Some("src/main.rs".to_string())
        );

        // Partial query
        assert_eq!(
            FuzzyFinderState::get_current_fuzzy_query(7, 10, content),
            Some("src".to_string())
        );

        // No query (cursor at trigger)
        assert_eq!(
            FuzzyFinderState::get_current_fuzzy_query(7, 7, content),
            None
        );
    }

    #[test]
    fn test_complete_fuzzy_finder() {
        let content = "Check @src here";
        let (new_content, row, col) =
            FuzzyFinderState::complete_fuzzy_finder(content, 7, 10, "src/main.rs");

        assert_eq!(new_content, "Check src/main.rs here");
        assert_eq!(row, 0);
        assert_eq!(col, 17);
    }

    #[test]
    fn test_complete_command_fuzzy() {
        let content = "/git status";
        let (new_content, row, col) =
            FuzzyFinderState::complete_command_fuzzy(content, 1, 11, "/git");

        assert_eq!(new_content, "/git");
        assert_eq!(row, 0);
        assert_eq!(col, 4);
    }
}
