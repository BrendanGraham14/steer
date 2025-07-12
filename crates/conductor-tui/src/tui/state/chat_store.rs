//! ChatStore - storage for the new ChatItem model

use crate::tui::model::{ChatItem, MessageRow, RowId};
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
    /// Current thread ID
    current_thread: Option<Uuid>,
    /// Fast lookup for pending tool calls (tool_id -> key)
    pending_tool_keys: HashMap<String, ChatItemKey>,
    /// Fast lookup for in-flight operations (operation_id -> key)
    in_flight_op_keys: HashMap<Uuid, ChatItemKey>,
}

impl Default for ChatStore {
    fn default() -> Self {
        Self {
            items: IndexMap::new(),
            id_to_key: HashMap::new(),
            next_key: 0,
            current_thread: None,
            pending_tool_keys: HashMap::new(),
            in_flight_op_keys: HashMap::new(),
        }
    }
}

impl ChatStore {
    /// Create an empty store
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current thread ID
    pub fn current_thread(&self) -> Option<Uuid> {
        self.current_thread
    }

    /// Set the current thread ID
    pub fn set_thread(&mut self, thread_id: Uuid) {
        self.current_thread = Some(thread_id);
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

        // If this is the first message and we don't have a thread set, use its thread
        if self.current_thread.is_none() {
            if let ChatItem::Message(ref row) = item {
                self.current_thread = Some(*row.inner.thread_id());
            }
        }

        // Track transient items for fast lookups
        match &item {
            ChatItem::PendingToolCall { tool_call, .. } => {
                self.pending_tool_keys.insert(tool_call.id.clone(), key);
            }
            ChatItem::InFlightOperation { operation_id, .. } => {
                self.in_flight_op_keys.insert(*operation_id, key);
            }
            _ => {}
        }

        self.items.insert(key, item);
        self.id_to_key.insert(id, key);
        key
    }

    /// Add a message row
    pub fn add_message(&mut self, message: Message) -> ChatItemKey {
        let row = MessageRow::new(message);

        if self.current_thread.is_none() {
            self.current_thread = Some(*row.inner.thread_id());
        } else {
            // If the new message is on a different thread, update the current thread
            // but do not clear the existing messages.
            let new_thread_id = *row.inner.thread_id();
            if self.current_thread != Some(new_thread_id) {
                self.current_thread = Some(new_thread_id);
            }
        }

        self.push(ChatItem::Message(row))
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

    /// Retain all messages that are either in the selected thread **or** are
    /// ancestors (by `parent_message_id`) of any message in that thread.
    ///
    /// This keeps the full conversation path leading up to the currently
    /// active branch while discarding unrelated sibling branches.  It is more
    /// robust than relying on only the latest message because some assistant
    /// messages can reference a parent ID that is missing from the store.
    pub fn prune_to_thread(&mut self, keep_thread_id: Uuid) {
        let mut live_ids = HashSet::new();
        let mut queue: Vec<String> = self
            .items
            .values()
            .filter_map(|item| match item {
                ChatItem::Message(row) if *row.inner.thread_id() == keep_thread_id => {
                    Some(row.inner.id().to_string())
                }
                _ => None,
            })
            .collect();

        let message_map: HashMap<String, &MessageRow> = self
            .items
            .values()
            .filter_map(|item| {
                if let ChatItem::Message(row) = item {
                    Some((row.inner.id().to_string(), row))
                } else {
                    None
                }
            })
            .collect();

        while let Some(id) = queue.pop() {
            if live_ids.contains(&id) {
                continue;
            }

            if let Some(row) = message_map.get(&id) {
                live_ids.insert(id.clone());
                if let Some(parent_id) = row.inner.parent_message_id() {
                    queue.push(parent_id.to_string());
                }
            }
        }

        // Remove items that are not in the live set
        let keys_to_remove: Vec<ChatItemKey> = self
            .items
            .iter()
            .filter_map(|(&key, item)| match item {
                ChatItem::Message(row) if !live_ids.contains(row.inner.id()) => Some(key),
                ChatItem::Message(_) => None,
                _ => None,
            })
            .collect();

        for key in keys_to_remove {
            self.remove_by_key(key);
        }

        self.current_thread = Some(keep_thread_id);
    }

    /// Find messages by parent ID
    pub fn find_by_parent(&self, parent_id: &str) -> Vec<&ChatItem> {
        self.items
            .values()
            .filter(|item| {
                if let ChatItem::Message(row) = item {
                    row.inner.parent_message_id() == Some(parent_id)
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
    pub fn messages(&self) -> Vec<&MessageRow> {
        self.items
            .values()
            .filter_map(|item| {
                if let ChatItem::Message(row) = item {
                    Some(row)
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
    pub fn user_messages(&self) -> Vec<(usize, &MessageRow)> {
        self.items
            .values()
            .enumerate()
            .filter_map(|(idx, item)| {
                if let ChatItem::Message(row) = item {
                    if matches!(row.inner, Message::User { .. }) {
                        Some((idx, row))
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
}
