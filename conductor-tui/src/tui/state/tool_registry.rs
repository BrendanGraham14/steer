//! ToolCallRegistry – phase-4 centralized tool call lifecycle tracking.
//!
//! This replaces the scattered `pending_tool_calls` and `tool_message_index` HashMaps
//! in `tui::Tui` with a centralized registry that tracks the complete lifecycle
//! of tool calls from registration through completion.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use conductor_core::app::conversation::ToolResult;
use tools::schema::ToolCall;

/// Status of a tool call in its lifecycle
#[derive(Debug, Clone, PartialEq)]
pub enum ToolStatus {
    /// Tool call has been registered but not started
    Pending,
    /// Tool call is currently executing
    Active,
    /// Tool call completed successfully
    Completed,
    /// Tool call failed or was cancelled
    Failed,
}

/// Information about a tool call at any stage in its lifecycle
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub call: ToolCall,
    pub status: ToolStatus,
    pub message_index: Option<usize>,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
    pub result: Option<ToolResult>,
}

/// State of an active tool call
#[derive(Debug, Clone)]
pub struct ToolCallState {
    pub call: ToolCall,
    pub status: ToolStatus,
    pub message_index: Option<usize>,
    pub started_at: Option<Instant>,
}

/// State of a completed tool call
#[derive(Debug, Clone)]
pub struct CompletedToolCall {
    pub call: ToolCall,
    pub result: ToolResult,
    pub message_index: Option<usize>,
    pub started_at: Option<Instant>,
    pub completed_at: Instant,
    pub duration: Option<Duration>,
}

/// Centralized registry for tracking tool call lifecycle.
///
/// This façade initially wraps the existing HashMap-based tracking but provides
/// a clean API for managing tool calls through their complete lifecycle.
#[derive(Debug, Default)]
pub struct ToolCallRegistry {
    /// Tool calls that have been registered but not started
    pending_calls: HashMap<String, ToolCall>,
    /// Tool calls currently executing
    active_calls: HashMap<String, ToolCallState>,
    /// Tool calls that have completed (success or failure)
    completed_calls: HashMap<String, CompletedToolCall>,
    /// Fast lookup from tool-id to message index for rendering
    tool_message_index: HashMap<String, usize>,
}

impl ToolCallRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new tool call (if not already present)
    pub fn register_call(&mut self, call: ToolCall) {
        let id = call.id.clone();

        tracing::debug!(
            target: "tool_registry",
            "register_call: id={}, name={}, params={}",
            id, call.name, call.parameters
        );

        if !self.pending_calls.contains_key(&id)
            && !self.active_calls.contains_key(&id)
            && !self.completed_calls.contains_key(&id)
        {
            self.pending_calls.insert(id, call);
        } else {
            tracing::debug!(
                target: "tool_registry",
                "register_call: Tool call {} already exists, skipping",
                id
            );
        }
    }

    /// Upsert a tool call – replaces existing call metadata in any stage
    /// without touching timing / status, or inserts as pending if not present.
    pub fn upsert_call(&mut self, call: ToolCall) {
        let id = call.id.clone();

        tracing::debug!(
            target: "tool_registry",
            "Upsert call: id={}, name={}, params={}",
            id, call.name, call.parameters
        );

        if let Some(p) = self.pending_calls.get_mut(&id) {
            tracing::debug!(target: "tool_registry", "Updating pending call {}", id);
            *p = call;
            return;
        }
        if let Some(state) = self.active_calls.get_mut(&id) {
            tracing::debug!(target: "tool_registry", "Updating active call {}", id);
            state.call = call;
            return;
        }
        if let Some(comp) = self.completed_calls.get_mut(&id) {
            tracing::debug!(target: "tool_registry", "Updating completed call {}", id);
            comp.call = call;
            return;
        }
        // Not present – register as new pending
        tracing::debug!(target: "tool_registry", "Inserting new pending call {}", id);
        self.pending_calls.insert(id, call);
    }

    /// Start execution of a registered tool call
    pub fn start_execution(&mut self, id: &str) -> Option<ToolCall> {
        if let Some(call) = self.pending_calls.remove(id) {
            let state = ToolCallState {
                call: call.clone(),
                status: ToolStatus::Active,
                message_index: self.tool_message_index.get(id).copied(),
                started_at: Some(Instant::now()),
            };
            self.active_calls.insert(id.to_string(), state);
            Some(call)
        } else {
            None
        }
    }

    /// Complete execution of a tool call with a result
    pub fn complete_execution(
        &mut self,
        id: &str,
        result: ToolResult,
    ) -> Option<CompletedToolCall> {
        if let Some(state) = self.active_calls.remove(id) {
            let completed_at = Instant::now();
            let duration = state
                .started_at
                .map(|started| completed_at.duration_since(started));

            let completed = CompletedToolCall {
                call: state.call,
                result,
                message_index: state.message_index,
                started_at: state.started_at,
                completed_at,
                duration,
            };

            self.completed_calls
                .insert(id.to_string(), completed.clone());
            Some(completed)
        } else {
            None
        }
    }

    /// Fail a tool call (removes it from active calls)
    pub fn fail_execution(&mut self, id: &str, error: String) -> Option<CompletedToolCall> {
        if let Some(state) = self.active_calls.remove(id) {
            let completed_at = Instant::now();
            let duration = state
                .started_at
                .map(|started| completed_at.duration_since(started));

            let completed = CompletedToolCall {
                call: state.call,
                result: ToolResult::Error { error },
                message_index: state.message_index,
                started_at: state.started_at,
                completed_at,
                duration,
            };

            self.completed_calls
                .insert(id.to_string(), completed.clone());
            Some(completed)
        } else {
            None
        }
    }

    /// Get information about a tool call at any stage
    pub fn get_call_info(&self, id: &str) -> Option<ToolCallInfo> {
        // Check completed first
        if let Some(completed) = self.completed_calls.get(id) {
            return Some(ToolCallInfo {
                call: completed.call.clone(),
                status: ToolStatus::Completed,
                message_index: completed.message_index,
                started_at: completed.started_at,
                completed_at: Some(completed.completed_at),
                result: Some(completed.result.clone()),
            });
        }

        // Check active
        if let Some(state) = self.active_calls.get(id) {
            return Some(ToolCallInfo {
                call: state.call.clone(),
                status: state.status.clone(),
                message_index: state.message_index,
                started_at: state.started_at,
                completed_at: None,
                result: None,
            });
        }

        // Check pending
        if let Some(call) = self.pending_calls.get(id) {
            return Some(ToolCallInfo {
                call: call.clone(),
                status: ToolStatus::Pending,
                message_index: self.tool_message_index.get(id).copied(),
                started_at: None,
                completed_at: None,
                result: None,
            });
        }

        None
    }

    /// Get a tool call from any stage (for backwards compatibility)
    pub fn get_tool_call(&self, id: &str) -> Option<&ToolCall> {
        let result = self
            .pending_calls
            .get(id)
            .or_else(|| self.active_calls.get(id).map(|s| &s.call))
            .or_else(|| self.completed_calls.get(id).map(|c| &c.call));

        if let Some(call) = result {
            tracing::debug!(
                target: "tool_registry",
                "Found tool call {}: name={}, params={}",
                id, call.name, call.parameters
            );
        } else {
            tracing::debug!(
                target: "tool_registry",
                "Tool call {} not found in registry",
                id
            );
        }

        result
    }

    /// Set the message index for a tool call (for rendering)
    pub fn set_message_index(&mut self, id: &str, index: usize) {
        self.tool_message_index.insert(id.to_string(), index);

        // Update active call state if it exists
        if let Some(state) = self.active_calls.get_mut(id) {
            state.message_index = Some(index);
        }
    }

    /// Get the message index for a tool call
    pub fn get_message_index(&self, id: &str) -> Option<usize> {
        self.tool_message_index.get(id).copied()
    }

    /// Check if a tool call is pending result
    pub fn is_pending_result(&self, id: &str) -> bool {
        self.active_calls.contains_key(id)
    }

    /// Get all pending tool calls (for backwards compatibility)
    pub fn pending_calls(&self) -> &HashMap<String, ToolCall> {
        &self.pending_calls
    }

    /// Get all active tool calls
    pub fn active_calls(&self) -> &HashMap<String, ToolCallState> {
        &self.active_calls
    }

    /// Get all completed tool calls
    pub fn completed_calls(&self) -> &HashMap<String, CompletedToolCall> {
        &self.completed_calls
    }

    /// Get the tool message index map (for backwards compatibility)
    pub fn tool_message_index(&self) -> &HashMap<String, usize> {
        &self.tool_message_index
    }

    /// Clear all registry state
    pub fn clear(&mut self) {
        self.pending_calls.clear();
        self.active_calls.clear();
        self.completed_calls.clear();
        self.tool_message_index.clear();
    }

    /// Get metrics about the registry state
    pub fn metrics(&self) -> ToolRegistryMetrics {
        ToolRegistryMetrics {
            pending_count: self.pending_calls.len(),
            active_count: self.active_calls.len(),
            completed_count: self.completed_calls.len(),
        }
    }

    /// Debug helper to dump registry state
    pub fn debug_dump(&self, prefix: &str) {
        tracing::debug!(
            target: "tool_registry",
            "{}: Registry state - pending: {}, active: {}, completed: {}",
            prefix,
            self.pending_calls.len(),
            self.active_calls.len(),
            self.completed_calls.len()
        );

        for (id, call) in &self.pending_calls {
            tracing::debug!(
                target: "tool_registry",
                "{}: Pending - id={}, name={}, params={}",
                prefix, id, call.name, call.parameters
            );
        }

        for (id, state) in &self.active_calls {
            tracing::debug!(
                target: "tool_registry",
                "{}: Active - id={}, name={}, params={}",
                prefix, id, state.call.name, state.call.parameters
            );
        }

        for (id, comp) in &self.completed_calls {
            tracing::debug!(
                target: "tool_registry",
                "{}: Completed - id={}, name={}, params={}",
                prefix, id, comp.call.name, comp.call.parameters
            );
        }
    }
}

/// Metrics about the tool registry state
#[derive(Debug, Clone)]
pub struct ToolRegistryMetrics {
    pub pending_count: usize,
    pub active_count: usize,
    pub completed_count: usize,
}
