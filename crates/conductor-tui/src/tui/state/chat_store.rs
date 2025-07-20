//! ChatStore - storage for the new ChatItem model

use crate::tui::model::{ChatItem, RowId};
use conductor_core::app::conversation::Message;

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Stable key for accessing chat items
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChatItemKey(u64);

/// Storage for chat items (messages and meta rows)
#[derive(Debug, Clone)]
pub struct ChatStore {
    /// All chat items stored in order with O(1) key-based access
    items: IndexMap<ChatItemKey, ChatItem>,
    /// Fast lookup id -> key
    id_to_key: HashMap<RowId, ChatItemKey>,
    /// Key generator
    next_key: u64,
    /// Fast lookup for pending tool calls (tool_id -> key)
    pending_tool_keys: HashMap<String, ChatItemKey>,
    /// Fast lookup for in-flight operations (operation_id -> key)
    in_flight_op_keys: HashMap<Uuid, ChatItemKey>,
    /// Revision number for dirty tracking
    revision: u64,
}

impl Default for ChatStore {
    fn default() -> Self {
        Self {
            items: IndexMap::new(),
            id_to_key: HashMap::new(),
            next_key: 0,
            pending_tool_keys: HashMap::new(),
            in_flight_op_keys: HashMap::new(),
            revision: 0,
        }
    }
}

impl ChatStore {
    /// Create an empty store
    pub fn new() -> Self {
        Self::default()
    }

    /// Get current revision number for dirty tracking
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Current number of items
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get all items as a vector of references
    pub fn as_vec(&self) -> Vec<&ChatItem> {
        self.items.values().collect()
    }

    /// Iterate over all items in order
    pub fn items(&self) -> impl Iterator<Item = &ChatItem> + '_ {
        self.items.values()
    }

    /// Get items as a slice for compatibility (allocates a Vec)
    pub fn to_vec(&self) -> Vec<ChatItem> {
        self.items.values().cloned().collect()
    }

    /// Borrow items as a vector for zero-copy iteration
    pub fn as_items(&self) -> Vec<&ChatItem> {
        self.items.values().collect()
    }

    /// Generate a new key
    fn generate_key(&mut self) -> ChatItemKey {
        let key = ChatItemKey(self.next_key);
        self.next_key += 1;
        key
    }

    /// Push a new item and return its key
    pub fn push(&mut self, item: ChatItem) -> ChatItemKey {
        let id = item.id().to_string();
        let key = self.generate_key();

        // Track transient items for fast lookups
        match &item {
            ChatItem::PendingToolCall { tool_call, .. } => {
                self.pending_tool_keys.insert(tool_call.id.clone(), key);
            }
            ChatItem::InFlightOperation { operation_id, .. } => {
                self.in_flight_op_keys.insert(*operation_id, key);
            }
            ChatItem::CoreCmdResponse { .. }
            | ChatItem::SystemNotice { .. }
            | ChatItem::Message(_)
            | ChatItem::SlashInput { .. }
            | ChatItem::TuiCommandResponse { .. } => {}
        }

        self.items.insert(key, item);
        self.id_to_key.insert(id, key);
        self.revision += 1; // Increment revision on mutation
        key
    }

    /// Add a message row
    pub fn add_message(&mut self, message: Message) -> ChatItemKey {
        self.push(ChatItem::Message(message))
    }

    /// Add a pending tool call
    pub fn add_pending_tool(&mut self, item: ChatItem) -> ChatItemKey {
        self.push(item)
    }

    /// Remove an item by index (for backwards compatibility)
    pub fn remove(&mut self, idx: usize) {
        if let Some((key, _)) = self.items.get_index(idx) {
            let key = *key;
            self.remove_by_key(key);
        }
    }

    /// Remove an item by its key
    pub fn remove_by_key(&mut self, key: ChatItemKey) {
        if let Some(item) = self.items.shift_remove(&key) {
            // Remove from id lookup
            self.id_to_key.remove(item.id());

            // Remove from transient tracking maps
            match &item {
                ChatItem::PendingToolCall { tool_call, .. } => {
                    self.pending_tool_keys.remove(&tool_call.id);
                }
                ChatItem::InFlightOperation { operation_id, .. } => {
                    self.in_flight_op_keys.remove(operation_id);
                }
                _ => {}
            }
            self.revision += 1; // Increment revision on mutation
        }
    }

    /// Remove an item by its ID
    pub fn remove_by_id(&mut self, id: &str) {
        if let Some(&key) = self.id_to_key.get(id) {
            self.remove_by_key(key);
        }
    }

    /// Clear all items
    pub fn clear(&mut self) {
        self.items.clear();
        self.id_to_key.clear();
        self.pending_tool_keys.clear();
        self.in_flight_op_keys.clear();
        self.revision += 1; // Increment revision on mutation
    }

    /// Get mutable reference by id
    pub fn get_mut_by_id(&mut self, id: &RowId) -> Option<&mut ChatItem> {
        let key = self.lookup(id)?;
        self.items.get_mut(&key)
    }

    /// Get immutable reference by id
    pub fn get_by_id(&self, id: &RowId) -> Option<&ChatItem> {
        let key = self.lookup(id)?;
        self.items.get(&key)
    }

    /// Retain only messages that are ancestors of the given message ID.
    /// This traverses the parent_message_id chain backwards from the given message
    /// and keeps only those messages in the lineage.
    pub fn prune_to_message_lineage(&mut self, keep_message_id: &str) {
        tracing::debug!(
            target: "chat_store",
            "Pruning to message lineage of: {}, current store size: {}",
            keep_message_id, self.items.len()
        );

        let mut live_ids = HashSet::new();

        // Build a map of message ID to message for quick lookups
        let message_map: HashMap<String, &Message> = self
            .items
            .values()
            .filter_map(|item| {
                if let ChatItem::Message(message) = item {
                    Some((message.id().to_string(), message))
                } else {
                    None
                }
            })
            .collect();

        // Start from the given message and traverse backwards
        let mut current_id = Some(keep_message_id.to_string());

        while let Some(id) = current_id {
            if live_ids.contains(&id) {
                break; // Avoid cycles
            }

            if let Some(message) = message_map.get(&id) {
                live_ids.insert(id.clone());
                current_id = message.parent_message_id().map(|s| s.to_string());
            } else {
                tracing::warn!(
                    target: "chat_store",
                    "Message not found during lineage traversal: {}",
                    id
                );
                break; // Message not found
            }
        }

        tracing::debug!(
            target: "chat_store",
            "Keeping {} messages in lineage",
            live_ids.len()
        );

        // Collect keys of messages to remove (those not in the lineage)
        let keys_to_remove: Vec<ChatItemKey> = self
            .items
            .iter()
            .filter_map(|(&key, item)| match item {
                ChatItem::Message(message) if !live_ids.contains(message.id()) => Some(key),
                _ => None,
            })
            .collect();

        // Remove items that are not in the lineage
        for key in keys_to_remove {
            self.remove_by_key(key);
        }

        tracing::debug!(
            target: "chat_store",
            "After pruning, store size: {}",
            self.items.len()
        );
    }

    /// Find messages by parent ID
    pub fn find_by_parent(&self, parent_id: &str) -> Vec<&ChatItem> {
        self.items
            .values()
            .filter(|item| {
                if let ChatItem::Message(message) = item {
                    message.parent_message_id() == Some(parent_id)
                } else {
                    false
                }
            })
            .collect()
    }

    /// Iterator over mutable items
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ChatItem> {
        self.items.values_mut()
    }

    /// Iterator over items
    pub fn iter(&self) -> impl Iterator<Item = &ChatItem> {
        self.items.values()
    }

    /// Direct access by index
    pub fn get(&self, idx: usize) -> Option<&ChatItem> {
        self.items.get_index(idx).map(|(_, item)| item)
    }

    /// Direct mutable access by index
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut ChatItem> {
        self.items.get_index_mut(idx).map(|(_, item)| item)
    }

    /// Get only message rows (filtering out meta rows)
    pub fn messages(&self) -> Vec<&Message> {
        self.items
            .values()
            .filter_map(|item| {
                if let ChatItem::Message(message) = item {
                    Some(message)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if there are any pending tool calls
    pub fn has_pending_tools(&self) -> bool {
        self.items
            .values()
            .any(|item| matches!(item, ChatItem::PendingToolCall { .. }))
    }

    /// Get user messages for edit history
    pub fn user_messages(&self) -> Vec<(usize, &Message)> {
        self.items
            .values()
            .enumerate()
            .filter_map(|(idx, item)| {
                if let ChatItem::Message(message) = item {
                    if matches!(message, Message::User { .. }) {
                        Some((idx, message))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    fn lookup(&self, id: &RowId) -> Option<ChatItemKey> {
        self.id_to_key.get(id).copied()
    }

    /// Iterator for items that can be used without allocating
    pub fn iter_items(&self) -> impl Iterator<Item = &ChatItem> + '_ {
        self.items.values()
    }

    /// Find an item by predicate (useful for finding pending tools, operations, etc.)
    pub fn find_item<F>(&self, predicate: F) -> Option<(ChatItemKey, &ChatItem)>
    where
        F: Fn(&ChatItem) -> bool,
    {
        self.items.iter().find_map(|(&key, item)| {
            if predicate(item) {
                Some((key, item))
            } else {
                None
            }
        })
    }

    /// Get pending tool call by ID
    pub fn get_pending_tool_key(&self, tool_id: &str) -> Option<ChatItemKey> {
        self.pending_tool_keys.get(tool_id).copied()
    }

    /// Remove pending tool call by ID
    pub fn remove_pending_tool(&mut self, tool_id: &str) {
        if let Some(key) = self.pending_tool_keys.get(tool_id).copied() {
            self.remove_by_key(key);
        }
    }

    /// Get in-flight operation by ID
    pub fn get_in_flight_op_key(&self, operation_id: &Uuid) -> Option<ChatItemKey> {
        self.in_flight_op_keys.get(operation_id).copied()
    }

    /// Remove in-flight operation by ID
    pub fn remove_in_flight_op(&mut self, operation_id: &Uuid) {
        if let Some(key) = self.in_flight_op_keys.get(operation_id).copied() {
            self.remove_by_key(key);
        }
    }

    /// Ingest multiple messages at once (used for conversation restoration)
    pub fn ingest_messages(&mut self, msgs: &[Message]) {
        for m in msgs {
            self.add_message(m.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::app::conversation::{AssistantContent, Message, UserContent};
    use conductor_tools::schema::ToolCall;
    use time::OffsetDateTime;

    fn create_test_message(id: &str, parent_id: Option<&str>) -> Message {
        Message::User {
            id: id.to_string(),
            content: vec![UserContent::Text {
                text: format!("Test message {id}"),
            }],
            timestamp: 1234567890,
            parent_message_id: parent_id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_prune_to_message_lineage_basic() {
        let mut store = ChatStore::new();

        // Create a simple linear chain: A -> B -> C
        store.add_message(create_test_message("A", None));
        store.add_message(create_test_message("B", Some("A")));
        store.add_message(create_test_message("C", Some("B")));

        assert_eq!(store.len(), 3);

        // Prune to keep only the lineage of C
        store.prune_to_message_lineage("C");

        // All messages should be kept
        assert_eq!(store.len(), 3);
        assert!(store.get_by_id(&"A".to_string()).is_some());
        assert!(store.get_by_id(&"B".to_string()).is_some());
        assert!(store.get_by_id(&"C".to_string()).is_some());
    }

    #[test]
    fn test_prune_to_message_lineage_with_branches() {
        let mut store = ChatStore::new();

        // Create a branching structure:
        //       A
        //      / \
        //     B   D
        //    /     \
        //   C       E
        store.add_message(create_test_message("A", None));
        store.add_message(create_test_message("B", Some("A")));
        store.add_message(create_test_message("C", Some("B")));
        store.add_message(create_test_message("D", Some("A")));
        store.add_message(create_test_message("E", Some("D")));

        assert_eq!(store.len(), 5);

        // Prune to keep only the lineage of C (A -> B -> C)
        store.prune_to_message_lineage("C");

        // Only A, B, C should remain
        assert_eq!(store.len(), 3);
        assert!(store.get_by_id(&"A".to_string()).is_some());
        assert!(store.get_by_id(&"B".to_string()).is_some());
        assert!(store.get_by_id(&"C".to_string()).is_some());
        assert!(store.get_by_id(&"D".to_string()).is_none());
        assert!(store.get_by_id(&"E".to_string()).is_none());
    }

    #[test]
    fn test_prune_to_message_lineage_preserves_non_messages() {
        let mut store = ChatStore::new();

        // Add messages
        store.add_message(create_test_message("A", None));
        store.add_message(create_test_message("B", Some("A")));
        store.add_message(create_test_message("C", Some("A"))); // Branch

        // Add non-message items
        let tool_call = ToolCall {
            id: "tool1".to_string(),
            name: "test_tool".to_string(),
            parameters: serde_json::json!({}),
        };
        store.push(ChatItem::PendingToolCall {
            id: "pending_tool_1".to_string(),
            tool_call,
            ts: OffsetDateTime::now_utc(),
        });

        let operation_id = Uuid::new_v4();
        store.push(ChatItem::InFlightOperation {
            id: format!("op_{operation_id}"),
            operation_id,
            label: "Test operation".to_string(),
            ts: OffsetDateTime::now_utc(),
        });

        assert_eq!(store.len(), 5); // 3 messages + 2 other items

        // Prune to keep only lineage of B
        store.prune_to_message_lineage("B");

        // Should have A, B, and the 2 non-message items
        assert_eq!(store.len(), 4);
        assert!(store.get_by_id(&"A".to_string()).is_some());
        assert!(store.get_by_id(&"B".to_string()).is_some());
        assert!(store.get_by_id(&"C".to_string()).is_none());

        // Verify non-message items are preserved
        assert!(store.get_pending_tool_key("tool1").is_some());
        assert!(store.get_in_flight_op_key(&operation_id).is_some());
    }

    #[test]
    fn test_prune_to_message_lineage_missing_parent() {
        let mut store = ChatStore::new();

        // Create messages with a missing parent
        store.add_message(create_test_message("B", Some("A"))); // A doesn't exist
        store.add_message(create_test_message("C", Some("B")));
        store.add_message(create_test_message("D", None)); // Unrelated message

        assert_eq!(store.len(), 3);

        // Prune to keep only lineage of C
        store.prune_to_message_lineage("C");

        // Should keep B and C (stops at missing parent A)
        assert_eq!(store.len(), 2);
        assert!(store.get_by_id(&"B".to_string()).is_some());
        assert!(store.get_by_id(&"C".to_string()).is_some());
        assert!(store.get_by_id(&"D".to_string()).is_none());
    }

    #[test]
    fn test_prune_to_message_lineage_nonexistent_message() {
        let mut store = ChatStore::new();

        store.add_message(create_test_message("A", None));
        store.add_message(create_test_message("B", Some("A")));

        assert_eq!(store.len(), 2);

        // Try to prune to a non-existent message
        store.prune_to_message_lineage("Z");

        // Nothing should be kept (except non-message items if any)
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_prune_to_message_lineage_complex_tree() {
        let mut store = ChatStore::new();

        // Create a more complex tree:
        //         A
        //       / | \
        //      B  C  D
        //     /       \
        //    E         F
        //   /           \
        //  G             H
        store.add_message(create_test_message("A", None));
        store.add_message(create_test_message("B", Some("A")));
        store.add_message(create_test_message("C", Some("A")));
        store.add_message(create_test_message("D", Some("A")));
        store.add_message(create_test_message("E", Some("B")));
        store.add_message(create_test_message("F", Some("D")));
        store.add_message(create_test_message("G", Some("E")));
        store.add_message(create_test_message("H", Some("F")));

        assert_eq!(store.len(), 8);

        // Prune to keep only lineage of G (A -> B -> E -> G)
        store.prune_to_message_lineage("G");

        assert_eq!(store.len(), 4);
        assert!(store.get_by_id(&"A".to_string()).is_some());
        assert!(store.get_by_id(&"B".to_string()).is_some());
        assert!(store.get_by_id(&"E".to_string()).is_some());
        assert!(store.get_by_id(&"G".to_string()).is_some());

        // Others should be removed
        assert!(store.get_by_id(&"C".to_string()).is_none());
        assert!(store.get_by_id(&"D".to_string()).is_none());
        assert!(store.get_by_id(&"F".to_string()).is_none());
        assert!(store.get_by_id(&"H".to_string()).is_none());
    }

    #[test]
    fn test_prune_to_message_lineage_assistant_messages() {
        let mut store = ChatStore::new();

        // Mix of user and assistant messages
        store.add_message(Message::User {
            id: "U1".to_string(),
            content: vec![UserContent::Text {
                text: "Hello".to_string(),
            }],
            timestamp: 1234567890,
            parent_message_id: None,
        });

        store.add_message(Message::Assistant {
            id: "A1".to_string(),
            content: vec![AssistantContent::Text {
                text: "Hi there!".to_string(),
            }],
            timestamp: 1234567891,
            parent_message_id: Some("U1".to_string()),
        });

        store.add_message(Message::User {
            id: "U2".to_string(),
            content: vec![UserContent::Text {
                text: "Another question".to_string(),
            }],
            timestamp: 1234567892,
            parent_message_id: Some("A1".to_string()),
        });

        // Branch from the first assistant message
        store.add_message(Message::User {
            id: "U3".to_string(),
            content: vec![UserContent::Text {
                text: "Different question".to_string(),
            }],
            timestamp: 1234567893,
            parent_message_id: Some("A1".to_string()),
        });

        assert_eq!(store.len(), 4);

        // Prune to keep only lineage of U2
        store.prune_to_message_lineage("U2");

        assert_eq!(store.len(), 3);
        assert!(store.get_by_id(&"U1".to_string()).is_some());
        assert!(store.get_by_id(&"A1".to_string()).is_some());
        assert!(store.get_by_id(&"U2".to_string()).is_some());
        assert!(store.get_by_id(&"U3".to_string()).is_none());
    }
}
