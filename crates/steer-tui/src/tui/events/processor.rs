use async_trait::async_trait;

use crate::tui::state::{ChatStore, ToolCallRegistry};
use crate::tui::widgets::ChatListState;
use steer_grpc::AgentClient;
use steer_grpc::client_api::{ClientEvent, ModelId, OpId, RequestId, ToolCall};

#[derive(Debug, Clone)]
#[expect(dead_code)]
pub enum ProcessingResult {
    Handled,
    HandledAndComplete,
    NotHandled,
    Failed(String),
}

/// Pending tool approval with request ID and tool call details
pub type PendingToolApproval = (RequestId, ToolCall);

#[expect(dead_code)]
pub struct ProcessingContext<'a> {
    pub chat_store: &'a mut ChatStore,
    pub chat_list_state: &'a mut ChatListState,
    pub tool_registry: &'a mut ToolCallRegistry,
    pub client: &'a AgentClient,
    pub is_processing: &'a mut bool,
    pub progress_message: &'a mut Option<String>,
    pub spinner_state: &'a mut usize,
    pub current_tool_approval: &'a mut Option<PendingToolApproval>,
    pub current_model: &'a mut ModelId,
    pub messages_updated: &'a mut bool,
    pub in_flight_operations: &'a mut std::collections::HashSet<OpId>,
    pub queued_head: &'a mut Option<steer_grpc::client_api::QueuedWorkItem>,
    pub queued_count: &'a mut usize,
}

#[async_trait]
pub trait EventProcessor: Send + Sync {
    fn priority(&self) -> usize {
        100
    }

    fn can_handle(&self, event: &ClientEvent) -> bool;

    async fn process(
        &mut self,
        event: ClientEvent,
        ctx: &mut ProcessingContext,
    ) -> ProcessingResult;

    fn name(&self) -> &'static str;
}
