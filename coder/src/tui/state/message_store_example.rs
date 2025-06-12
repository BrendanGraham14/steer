// Example implementation of MessageStore to demonstrate the concept

use std::collections::HashMap;
use crate::app::{Message, conversation::{AssistantContent, ToolResult}};
use crate::tui::widgets::message_list::MessageContent;
use tools::ToolCall;

/// Centralized store for managing all message-related state in the TUI
pub struct MessageStore {
    /// All messages in display order
    messages: Vec<MessageContent>,
    
    /// Tool calls extracted from assistant messages, keyed by tool call ID
    tool_calls: HashMap<String, ToolCall>,
    
    /// Message ID to index mapping for quick lookups
    message_index: HashMap<String, usize>,
    
    /// Tool call ID to message index mapping
    tool_message_index: HashMap<String, usize>,
    
    /// Tracks which tool calls are pending results
    pending_tool_results: HashSet<String>,
}

/// Describes what changed after a store mutation
#[derive(Debug, Clone)]
pub enum MessageChange {
    /// A new message was added at the given index
    Added { index: usize },
    
    /// An existing message was updated at the given index
    Updated { index: usize },
    
    /// A tool result was merged into an existing tool message
    ToolResultAdded { index: usize, tool_id: String },
    
    /// Multiple changes occurred
    Batch(Vec<MessageChange>),
}

impl MessageStore {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            tool_calls: HashMap::new(),
            message_index: HashMap::new(),
            tool_message_index: HashMap::new(),
            pending_tool_results: HashSet::new(),
        }
    }
    
    /// Add a message from the app layer
    pub fn add_message(&mut self, message: Message) -> MessageChange {
        match message {
            Message::Assistant { ref content, .. } => {
                // First, extract any tool calls for our registry
                let mut extracted_tool_calls = Vec::new();
                for block in content {
                    if let AssistantContent::ToolCall { tool_call } = block {
                        self.tool_calls.insert(tool_call.id.clone(), tool_call.clone());
                        self.pending_tool_results.insert(tool_call.id.clone());
                        extracted_tool_calls.push(tool_call.id.clone());
                    }
                }
                
                // Convert and add the message
                let content = self.convert_message(message);
                let index = self.messages.len();
                let id = content.id().to_string();
                
                self.messages.push(content);
                self.message_index.insert(id, index);
                
                // If we extracted tool calls, we might need to create placeholder tool messages
                if !extracted_tool_calls.is_empty() {
                    let mut changes = vec![MessageChange::Added { index }];
                    
                    for tool_id in extracted_tool_calls {
                        // Check if we already have a tool message for this ID
                        if !self.tool_message_index.contains_key(&tool_id) {
                            // Create placeholder
                            let placeholder = self.create_tool_placeholder(&tool_id);
                            let tool_index = self.messages.len();
                            self.messages.push(placeholder);
                            self.tool_message_index.insert(tool_id, tool_index);
                            changes.push(MessageChange::Added { index: tool_index });
                        }
                    }
                    
                    MessageChange::Batch(changes)
                } else {
                    MessageChange::Added { index }
                }
            }
            
            Message::Tool { ref tool_use_id, ref result, .. } => {
                // Check if we have the tool call info
                if let Some(existing_idx) = self.tool_message_index.get(tool_use_id) {
                    // Update existing tool message with result
                    if let MessageContent::Tool { result: ref mut existing_result, .. } = 
                        &mut self.messages[*existing_idx] {
                        *existing_result = Some(result.clone());
                        self.pending_tool_results.remove(tool_use_id);
                        MessageChange::ToolResultAdded { 
                            index: *existing_idx, 
                            tool_id: tool_use_id.clone() 
                        }
                    } else {
                        // Shouldn't happen, but handle gracefully
                        self.add_orphan_tool_result(tool_use_id, result)
                    }
                } else {
                    // No existing tool message, create one
                    self.add_orphan_tool_result(tool_use_id, result)
                }
            }
            
            _ => {
                // User messages and others
                let content = self.convert_message(message);
                let index = self.messages.len();
                let id = content.id().to_string();
                
                self.messages.push(content);
                self.message_index.insert(id, index);
                
                MessageChange::Added { index }
            }
        }
    }
    
    /// Update a tool call's status or result
    pub fn update_tool_result(&mut self, tool_id: &str, result: ToolResult) -> Option<MessageChange> {
        if let Some(&index) = self.tool_message_index.get(tool_id) {
            if let MessageContent::Tool { result: ref mut existing_result, .. } = 
                &mut self.messages[index] {
                *existing_result = Some(result);
                self.pending_tool_results.remove(tool_id);
                Some(MessageChange::ToolResultAdded { 
                    index, 
                    tool_id: tool_id.to_string() 
                })
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Get all messages for display
    pub fn get_messages(&self) -> &[MessageContent] {
        &self.messages
    }
    
    /// Find a specific tool call by ID
    pub fn find_tool_call(&self, id: &str) -> Option<&ToolCall> {
        self.tool_calls.get(id)
    }
    
    /// Check if we're waiting for a tool result
    pub fn is_pending_result(&self, tool_id: &str) -> bool {
        self.pending_tool_results.contains(tool_id)
    }
    
    /// Get message by ID
    pub fn get_message_by_id(&self, id: &str) -> Option<&MessageContent> {
        self.message_index.get(id).map(|&idx| &self.messages[idx])
    }
    
    // Private helper methods
    
    fn convert_message(&self, message: Message) -> MessageContent {
        // Implementation would use the existing conversion logic
        // but with access to self.tool_calls for lookups
        todo!("Implement using existing conversion logic")
    }
    
    fn create_tool_placeholder(&self, tool_id: &str) -> MessageContent {
        let tool_call = self.tool_calls.get(tool_id)
            .cloned()
            .unwrap_or_else(|| ToolCall {
                id: tool_id.to_string(),
                name: "unknown".to_string(),
                parameters: serde_json::Value::Null,
            });
            
        MessageContent::Tool {
            id: tool_id.to_string(),
            call: tool_call,
            result: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
    
    fn add_orphan_tool_result(&mut self, tool_id: &str, result: &ToolResult) -> MessageChange {
        // Create a tool message without the original call info
        let tool_call = ToolCall {
            id: tool_id.to_string(),
            name: "unknown".to_string(),
            parameters: serde_json::Value::Null,
        };
        
        let content = MessageContent::Tool {
            id: tool_id.to_string(),
            call: tool_call,
            result: Some(result.clone()),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        
        let index = self.messages.len();
        self.messages.push(content);
        self.tool_message_index.insert(tool_id.to_string(), index);
        
        MessageChange::Added { index }
    }
}

// Usage example in the TUI:
impl Tui {
    async fn handle_app_event_refactored(&mut self, event: AppEvent) {
        let change = match event {
            AppEvent::MessageAdded { message, .. } => {
                Some(self.message_store.add_message(message))
            }
            
            AppEvent::ToolCallCompleted { id, result, .. } => {
                let tool_result = ToolResult::Success { output: result };
                self.message_store.update_tool_result(&id, tool_result)
            }
            
            _ => None,
        };
        
        // Handle view updates based on changes
        if let Some(change) = change {
            self.handle_message_change(change);
        }
    }
    
    fn handle_message_change(&mut self, change: MessageChange) {
        match change {
            MessageChange::Added { .. } => {
                // Auto-scroll to bottom if near bottom
                self.message_list_state.invalidate_cache();
            }
            MessageChange::Updated { .. } => {
                self.message_list_state.invalidate_cache();
            }
            MessageChange::ToolResultAdded { .. } => {
                // Could play a sound or show notification
                self.message_list_state.invalidate_cache();
            }
            MessageChange::Batch(changes) => {
                for change in changes {
                    self.handle_message_change(change);
                }
            }
        }
    }
}