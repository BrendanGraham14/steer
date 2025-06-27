//! ChatStore - storage for the new ChatItem model

use crate::tui::model::{ChatItem, MessageRow, RowId};
use conductor_core::app::conversation::Message;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Storage for chat items (messages and meta rows)
#[derive(Debug, Default, Clone)]
pub struct ChatStore {
    /// All chat items for the current thread
    items: Vec<ChatItem>,
    /// Fast lookup id -> index
    index: HashMap<RowId, usize>,
    /// Current thread ID
    current_thread: Option<Uuid>,
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

    /// Immutable slice of all items
    pub fn as_slice(&self) -> &[ChatItem] {
        &self.items
    }

    /// Push a new item and return its index
    pub fn push(&mut self, item: ChatItem) -> usize {
        let idx = self.items.len();
        self.index.insert(item.id().to_string(), idx);

        // If this is the first message and we don't have a thread set, use its thread
        if self.current_thread.is_none() {
            if let ChatItem::Message(row) = &item {
                self.current_thread = Some(*row.inner.thread_id());
            }
        }

        self.items.push(item);
        idx
    }

    /// Add a message row
    pub fn add_message(&mut self, message: Message) -> usize {
        let row = MessageRow { inner: message };

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
    pub fn add_pending_tool(&mut self, item: ChatItem) -> usize {
        self.push(item)
    }

    /// Remove an item by index
    pub fn remove(&mut self, idx: usize) {
        if idx < self.items.len() {
            let item = self.items.remove(idx);
            // Remove from index
            self.index.remove(item.id());
            // Rebuild index to fix indices after removal
            self.rebuild_index();
        }
    }

    /// Clear all items
    pub fn clear(&mut self) {
        self.items.clear();
        self.index.clear();
    }

    /// Get mutable reference by id
    pub fn get_mut_by_id(&mut self, id: &RowId) -> Option<&mut ChatItem> {
        let idx = self.lookup(id)?;
        self.items.get_mut(idx)
    }

    /// Get immutable reference by id
    pub fn get_by_id(&self, id: &RowId) -> Option<&ChatItem> {
        let idx = self.lookup(id)?;
        self.items.get(idx)
    }

    /// Retain all messages that are either in the selected thread **or** are
    /// ancestors (by `parent_message_id`) of any message in that thread.
    ///
    /// This keeps the full conversation path leading up to the currently
    /// active branch while discarding unrelated sibling branches.  It is more
    /// robust than relying on only the latest message because some assistant
    /// messages can reference a parent ID that is missing from the store.
    pub fn prune_to_thread(&mut self, keep_thread_id: Uuid) {
        // 1. Collect every message that belongs to the target thread.
        let mut queue: Vec<String> = self
            .items
            .iter()
            .filter_map(|item| match item {
                ChatItem::Message(row) if *row.inner.thread_id() == keep_thread_id => {
                    Some(row.inner.id().to_string())
                }
                _ => None,
            })
            .collect();

        // 2. Walk the ancestor chain for each message, accumulating ids that
        // should stay alive.
        let mut live_ids: HashSet<String> = HashSet::new();
        while let Some(id) = queue.pop() {
            if !live_ids.insert(id.clone()) {
                // already processed
                continue;
            }
            // Find the message row for this id so we can follow its parent.
            if let Some(parent_id) = self.items.iter().find_map(|item| match item {
                ChatItem::Message(row) if row.inner.id() == id => row.inner.parent_message_id(),
                _ => None,
            }) {
                queue.push(parent_id.to_string());
            }
        }

        // 3. Retain messages that are in live_ids OR have the keep_thread_id.
        self.items.retain(|item| match item {
            ChatItem::Message(row) => {
                live_ids.contains(row.inner.id()) || *row.inner.thread_id() == keep_thread_id
            }
            _ => true, // keep meta rows
        });

        self.rebuild_index();
        self.current_thread = Some(keep_thread_id);
    }

    /// Find messages by parent ID
    pub fn find_by_parent(&self, parent_id: &str) -> Vec<&ChatItem> {
        self.items
            .iter()
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
        self.items.iter_mut()
    }

    /// Iterator over items
    pub fn iter(&self) -> impl Iterator<Item = &ChatItem> {
        self.items.iter()
    }

    /// Direct access by index
    pub fn get(&self, idx: usize) -> Option<&ChatItem> {
        self.items.get(idx)
    }

    /// Direct mutable access by index
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut ChatItem> {
        self.items.get_mut(idx)
    }

    /// Get only message rows (filtering out meta rows)
    pub fn messages(&self) -> Vec<&MessageRow> {
        self.items
            .iter()
            .filter_map(|item| {
                if let ChatItem::Message(row) = item {
                    Some(row)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get user messages for edit history
    pub fn user_messages(&self) -> Vec<(usize, &MessageRow)> {
        self.items
            .iter()
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

    /// Rebuild the index after direct mutations
    pub fn rebuild_index(&mut self) {
        self.index.clear();
        for (idx, item) in self.items.iter().enumerate() {
            self.index.insert(item.id().to_string(), idx);
        }
    }

    fn lookup(&self, id: &RowId) -> Option<usize> {
        self.index.get(id).copied()
    }
}

// Vec-like convenience impls
impl std::ops::Deref for ChatStore {
    type Target = Vec<ChatItem>;

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl std::ops::DerefMut for ChatStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.items
    }
}
