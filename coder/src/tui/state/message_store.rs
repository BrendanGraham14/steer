//! MessageStore façade – phase-1 minimal wrapper.
//!
//! This is **phase 1** of the TUI refactor (see `TUI_REFACTOR_PROPOSAL.md`).
//! We introduce a `MessageStore` that **wraps** the existing `Vec<MessageContent>`
//! used in `tui::Tui` without changing behaviour. Subsequent PRs will migrate
//! logic into this type.

use std::collections::HashMap;

use crate::tui::widgets::message_list::MessageContent;

/// Minimal façade around the message vector used by `tui::Tui`.
/// No behaviour change – offers the same public API that Tui used directly on
/// the `Vec<MessageContent>` today (push, indexing, iteration, len, etc.).
#[derive(Debug, Default, Clone)]
pub struct MessageStore {
    messages: Vec<MessageContent>,
    // fast lookup id -> index, populated lazily on demand for O(1) gets.
    index: HashMap<String, usize>,
}

impl MessageStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Immutable slice of all messages (for rendering).
    pub fn as_slice(&self) -> &[MessageContent] {
        &self.messages
    }

    /// Mutable slice if caller needs direct vector access (phase-1 only).
    pub fn as_mut_slice(&mut self) -> &mut [MessageContent] {
        &mut self.messages
    }

    /// Push a new message and return its index.
    pub fn push(&mut self, message: MessageContent) -> usize {
        let idx = self.messages.len();
        self.index.insert(message.id().to_string(), idx);
        self.messages.push(message);
        idx
    }
    
    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
        self.index.clear();
    }

    /// Get mutable reference by id, if it exists.
    pub fn get_mut_by_id(&mut self, id: &str) -> Option<&mut MessageContent> {
        let idx = self.lookup(id)?;
        self.messages.get_mut(idx)
    }

    /// Get immutable reference by id, if it exists.
    pub fn get_by_id(&self, id: &str) -> Option<&MessageContent> {
        let idx = self.lookup(id)?;
        self.messages.get(idx)
    }

    /// Find the first message matching predicate, returning mutable reference
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut MessageContent> {
        self.messages.iter_mut()
    }

    /// Find the first message matching predicate, returning immutable reference
    pub fn iter(&self) -> impl Iterator<Item = &MessageContent> {
        self.messages.iter()
    }

    /// Direct access by index
    pub fn get(&self, idx: usize) -> Option<&MessageContent> {
        self.messages.get(idx)
    }

    /// Direct mutable access by index
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut MessageContent> {
        self.messages.get_mut(idx)
    }

    /// Rebuild the index after direct mutations (call when messages have been modified via deref)
    pub fn rebuild_index(&mut self) {
        self.index.clear();
        for (idx, msg) in self.messages.iter().enumerate() {
            self.index.insert(msg.id().to_string(), idx);
        }
    }

    fn lookup(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
}

// Vec-like convenience impls -------------------------------------------------

impl std::ops::Deref for MessageStore {
    type Target = Vec<MessageContent>;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

impl std::ops::DerefMut for MessageStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.messages
    }
}
