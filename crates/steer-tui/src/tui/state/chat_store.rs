//! ChatStore - storage for the new ChatItem model

use crate::tui::model::{ChatItem, ChatItemData, RowId};
use steer_grpc::client_api::{Message, MessageData};

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Stable key for accessing chat items
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    /// Currently active message ID (for branch filtering)
    active_message_id: Option<String>,
    /// Message key before which items are hidden due to compaction
    compaction_head_key: Option<ChatItemKey>,
    /// Message IDs that are compaction summaries (for separator rendering)
    compaction_summary_ids: HashSet<String>,
    /// Mapping from compaction summary ID -> compacted head message ID.
    compaction_summary_heads: HashMap<String, String>,
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
            active_message_id: None,
            compaction_head_key: None,
            compaction_summary_ids: HashSet::new(),
            compaction_summary_heads: HashMap::new(),
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

    /// Set the active message ID
    pub fn set_active_message_id(&mut self, id: Option<String>) {
        self.active_message_id = id;
        self.revision += 1;
    }

    /// Set the compaction head message ID; messages at or before this key are hidden.
    pub fn set_compaction_head(&mut self, id: Option<String>) {
        self.compaction_head_key = id.as_ref().and_then(|id| self.lookup(id));
        self.revision += 1;
    }

    /// Get the active message ID
    pub fn active_message_id(&self) -> Option<&String> {
        self.active_message_id.as_ref()
    }

    pub fn compaction_head_key(&self) -> Option<ChatItemKey> {
        self.compaction_head_key
    }

    /// Mark a message as a compaction summary (for separator rendering).
    pub fn mark_compaction_summary(&mut self, id: String) {
        self.mark_compaction_summary_with_head(id, None);
    }

    /// Mark a compaction summary and optionally remember the compacted head ID.
    pub fn mark_compaction_summary_with_head(
        &mut self,
        summary_id: String,
        compacted_head_message_id: Option<String>,
    ) {
        self.compaction_summary_ids.insert(summary_id.clone());
        if let Some(compacted_head_message_id) = compacted_head_message_id {
            self.compaction_summary_heads
                .insert(summary_id, compacted_head_message_id);
        } else {
            self.compaction_summary_heads.remove(&summary_id);
        }
        self.revision += 1;
    }

    /// Check if a message is a compaction summary.
    pub fn is_compaction_summary(&self, id: &str) -> bool {
        self.compaction_summary_ids.contains(id)
    }

    /// Resolve the compacted head message ID for a compaction summary, if known.
    pub fn compacted_head_for_summary(&self, id: &str) -> Option<&str> {
        self.compaction_summary_heads.get(id).map(String::as_str)
    }

    /// Push a new item and return its key
    pub fn push(&mut self, mut item: ChatItem) -> ChatItemKey {
        let id = item.id().to_string();
        let key = self.generate_key();

        // For non-message items without a parent, set parent_chat_item_id to active_message_id
        if !matches!(item.data, ChatItemData::Message(_)) && item.parent_chat_item_id.is_none() {
            item.parent_chat_item_id.clone_from(&self.active_message_id);
        }

        // Track transient items for fast lookups
        match &item.data {
            ChatItemData::PendingToolCall { tool_call, .. } => {
                self.pending_tool_keys.insert(tool_call.id.clone(), key);
            }
            ChatItemData::InFlightOperation { operation_id, .. } => {
                self.in_flight_op_keys.insert(*operation_id, key);
            }
            ChatItemData::CoreCmdResponse { .. }
            | ChatItemData::SystemNotice { .. }
            | ChatItemData::Message(_)
            | ChatItemData::SlashInput { .. }
            | ChatItemData::TuiCommandResponse { .. } => {}
        }

        self.items.insert(key, item);
        self.id_to_key.insert(id, key);
        self.revision += 1; // Increment revision on mutation
        key
    }

    /// Add a message row
    pub fn add_message(&mut self, message: Message) -> ChatItemKey {
        self.push(ChatItem {
            parent_chat_item_id: None, // Messages have their own parent_message_id
            data: ChatItemData::Message(message),
        })
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
            match &item.data {
                ChatItemData::PendingToolCall { tool_call, .. } => {
                    self.pending_tool_keys.remove(&tool_call.id);
                }
                ChatItemData::InFlightOperation { operation_id, .. } => {
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
        self.compaction_head_key = None;
        self.compaction_summary_ids.clear();
        self.compaction_summary_heads.clear();
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

    /// Find messages by parent ID
    pub fn find_by_parent(&self, parent_id: &str) -> Vec<&ChatItem> {
        self.items
            .values()
            .filter(|item| {
                if let ChatItemData::Message(message) = &item.data {
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
                if let ChatItemData::Message(message) = &item.data {
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
            .any(|item| matches!(item.data, ChatItemData::PendingToolCall { .. }))
    }

    /// Get user messages for edit history
    pub fn user_messages(&self) -> Vec<(usize, &Message)> {
        self.items
            .values()
            .enumerate()
            .filter_map(|(idx, item)| {
                if let ChatItemData::Message(message) = &item.data {
                    if matches!(&message.data, MessageData::User { .. }) {
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

    pub fn is_before_compaction(&self, id: &str) -> bool {
        let Some(head_key) = self.compaction_head_key else {
            return false;
        };
        let Some(key) = self.lookup(&id.to_string()) else {
            return false;
        };
        key <= head_key
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

    /// Build the lineage set by following parent_message_id chain backwards from active_message_id
    pub fn build_lineage_set(&self) -> HashSet<String> {
        let mut lineage = HashSet::new();
        let Some(active_id) = &self.active_message_id else {
            for item in self.items.values() {
                if let ChatItemData::Message(msg) = &item.data {
                    lineage.insert(msg.id().to_string());
                }
            }
            return lineage;
        };

        let mut current = Some(active_id.clone());
        while let Some(id) = current {
            lineage.insert(id.clone());

            current = self.get_by_id(&id).and_then(|item| {
                if let ChatItemData::Message(msg) = &item.data {
                    msg.parent_message_id().map(|s| s.to_string())
                } else {
                    None
                }
            });
        }

        lineage
    }

    /// Get user messages that are in the active branch lineage
    pub fn user_messages_in_lineage(&self) -> Vec<(String, String)> {
        let lineage = self.build_lineage_set();

        self.items
            .values()
            .filter_map(|item| {
                if let ChatItemData::Message(message) = &item.data {
                    if matches!(&message.data, MessageData::User { .. })
                        && lineage.contains(message.id())
                        && !self.is_before_compaction(message.id())
                    {
                        Some((message.id().to_string(), message.content_string()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use steer_grpc::client_api::{AssistantContent, UserContent};

    fn user_message(id: &str, parent_id: Option<&str>, text: &str) -> Message {
        Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: text.to_string(),
                }],
            },
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            id: id.to_string(),
            parent_message_id: parent_id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_user_messages_in_lineage_filters_branch() {
        let mut store = ChatStore::new();
        store.add_message(user_message("msg1", None, "Root"));
        store.add_message(user_message("msg2", Some("msg1"), "Branch A"));
        store.add_message(user_message("msg3", Some("msg1"), "Branch B"));

        store.set_active_message_id(Some("msg2".to_string()));

        let messages = store.user_messages_in_lineage();
        let ids: Vec<_> = messages.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["msg1", "msg2"]);
    }

    #[test]
    fn test_user_messages_in_lineage_without_active() {
        let mut store = ChatStore::new();
        store.add_message(user_message("msg1", None, "Root"));
        store.add_message(user_message("msg2", Some("msg1"), "Branch A"));
        store.add_message(user_message("msg3", Some("msg1"), "Branch B"));

        let messages = store.user_messages_in_lineage();
        let ids: Vec<_> = messages.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["msg1", "msg2", "msg3"]);
    }

    #[test]
    fn test_compacted_head_for_summary_round_trip() {
        let mut store = ChatStore::new();

        store.mark_compaction_summary_with_head(
            "summary-msg".to_string(),
            Some("compacted-head".to_string()),
        );

        assert_eq!(
            store.compacted_head_for_summary("summary-msg"),
            Some("compacted-head")
        );
    }

    #[test]
    fn test_mark_compaction_summary_without_head_clears_mapping() {
        let mut store = ChatStore::new();

        store.mark_compaction_summary_with_head(
            "summary-msg".to_string(),
            Some("compacted-head".to_string()),
        );
        store.mark_compaction_summary("summary-msg".to_string());

        assert!(store.is_compaction_summary("summary-msg"));
        assert_eq!(store.compacted_head_for_summary("summary-msg"), None);
    }

    #[test]
    fn test_clear_resets_compaction_summary_state() {
        let mut store = ChatStore::new();

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "summary".to_string(),
                }],
            },
            timestamp: 1000,
            id: "summary-msg".to_string(),
            parent_message_id: None,
        });
        store.mark_compaction_summary_with_head(
            "summary-msg".to_string(),
            Some("compacted-head".to_string()),
        );

        store.clear();

        assert!(!store.is_compaction_summary("summary-msg"));
        assert_eq!(store.compacted_head_for_summary("summary-msg"), None);
    }
}
