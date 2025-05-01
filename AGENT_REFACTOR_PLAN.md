# Refined Plan: Event-Driven Agent Executor Service

This plan outlines the decoupling of the agent interaction loop from the main application logic (`src/app/mod.rs`) into a dedicated `AgentExecutor` service.

## Core Concepts

1.  **AgentExecutor Struct:**
    *   A relatively lightweight struct, possibly created once at application startup.
    *   Primarily holds shared dependencies like the `ApiClient`.
    *   The main logic resides within a dedicated method for running a single agent operation.

2.  **`run_operation` Method:**
    *   The core asynchronous method responsible for handling a complete agent interaction sequence (e.g., responding to a user message).
    *   This method encapsulates the full loop of API calls and tool execution.
    *   Signature (Conceptual):
        ```rust
        async fn run_operation<F, Fut>(
            &self, // Contains ApiClient
            model: Model,
            initial_messages: Vec<ApiMessage>,
            system_prompt: Option<String>,
            available_tools: Vec<ApiToolDefinition>, // Definitions for the API
            tool_executor_callback: F, // Async callback provided by App
            event_sender: mpsc::Sender<AgentEvent>, // Channel for events
            approval_mode: ApprovalMode, // NEW: Controls tool approval flow
            token: CancellationToken,
        ) -> Result<ApiMessage> // Returns the *final* assistant message
        where
            F: Fn(ApiToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<String, ToolError>> + Send + 'static;
        ```
    *   Receives all necessary context *per execution*, including:
        *   The specific `Model`, `initial_Messages`, `system_Prompt`, `available_Tools`.
        *   An asynchronous `tool_executor_callback` provided by the `App` layer (see section below).
        *   An `event_sender` channel (`tokio::sync::mpsc::Sender`) for progress events.
        *   An `ApprovalMode` (either `Automatic` or `Interactive`) to control how tool calls are approved.
        *   A `CancellationToken`.
    *   Handles runtime changes (model, tools) naturally as they are passed per-call.
    *   Returns the final assistant message or an error upon completion.

3.  **Internal Loop & Tool Handling:**
    *   `run_operation` manages the loop: Call LLM -> Parse Response -> Handle Tools -> Repeat/Finish.
    *   Emits text progress via `AgentEvent::AssistantMessagePart` / `AgentEvent::AssistantMessageFinal`.
    *   **Tool Call Handling:** Depends on `approval_mode`:
        *   **`ApprovalMode::Automatic`:**
            *   Iterates through tool calls received from LLM.
            *   Invokes the `tool_executor_callback` for each tool call.
            *   Emits `AgentEvent::ExecutingTool` / `AgentEvent::ToolResultReceived` around the callback execution.
            *   Formats results and sends back to LLM.
        *   **`ApprovalMode::Interactive`:** (Detailed in section "Interactive Tool Approval")
            *   Pauses execution.
            *   Initiates a request/response flow with the `App` layer via dedicated channels to get approval for each tool individually.
            *   Executes *only* the approved tools via the `tool_executor_callback`.
            *   Formats results (approved + placeholders for denied) and sends back to LLM.

4.  **Dynamic Tools & `tool_executor_callback`:**
    *   The `App` layer determines the `available_tools` list *before* calling `run_operation`, allowing dynamic sources (e.g., MCP `tools/list`).
    *   The `tool_executor_callback` provided by the `App` encapsulates the actual tool execution logic.
    *   **Implementation:** This callback will typically capture an appropriate `Arc<ToolExecutor>` instance (either the standard one or a `ToolExecutor::read_only()` instance) and delegate the execution by calling its `execute_tool_with_cancellation` method.

5.  **Event-Driven Communication & Approval Flow:**
    *   Uses `tokio::sync::mpsc` for `AgentEvent`s from `AgentExecutor` to `App`.
    *   **`AgentEvent` Enum (Key Events):**
        ```rust
        enum ApprovalDecision { Approved, Denied }

        // NEW: Sent from AgentExecutor -> App in Interactive mode
        // Contains the tool call details and a oneshot channel sender
        // for the App to send the decision back.
        struct ToolApprovalRequest {
            pub index: usize, // Original order index
            pub tool_call: ApiToolCall,
            pub responder: tokio::sync::oneshot::Sender<ApprovalDecision>,
        }

        enum AgentEvent {
            AssistantMessagePart(String),
            AssistantMessageFinal(ApiMessage),
            // Event specific to interactive approval:
            // Contains a list of all tools needing approval for this turn.
            RequestToolApprovals(Vec<ToolApprovalRequest>),
            ExecutingTool { tool_call_id: String, name: String },
            ToolResultReceived(ToolResult),
            // ... other potential events
        }
        ```
    *   **Interactive Approval Mechanism (`ApprovalMode::Interactive`):**
        1.  `AgentExecutor` gets tool calls from LLM.
        2.  For each `ApiToolCall`, creates a `tokio::sync::oneshot` channel (`sender`, `receiver`).
        3.  Collects `ToolApprovalRequest` structs (containing the `tool_call` and the `sender`) for all pending tools.
        4.  Sends a *single* `AgentEvent::RequestToolApprovals` event containing the `Vec<ToolApprovalRequest>` to the `App`.
        5.  Pauses execution, waiting asynchronously for all `receiver`s using `futures::future::join_all`.
        6.  `App` receives the `RequestToolApprovals` event.
        7.  Stores the `responder` (`oneshot::Sender`) for each tool call, likely in a `HashMap<tool_call_id, oneshot::Sender>`.
        8.  For each request, `App` emits an `AppEvent` to the UI layer to prompt the user for an `ApprovalDecision`.
        9.  When the UI layer responds (e.g., via an `AppCommand`), the `App` looks up the corresponding `responder` and sends the `ApprovalDecision` back through it.
        10. `AgentExecutor`'s `join_all` call unblocks as decisions arrive. It receives all decisions (or errors if a channel is dropped).
        11. `AgentExecutor` resumes, executes approved tools via the callback, formats results (including placeholders for denied/failed tools), and continues the loop.

## Application Layer (`App`) Responsibilities

*   Instantiate `AgentExecutor`.
*   For each agent operation:
    *   Prepare `model`, `messages`, `system_prompt`, `token`, `approval_mode`.
    *   Determine `available_tools`.
    *   Define the `tool_executor_callback` async closure (capturing the appropriate `ToolExecutor`).
    *   Create the main `mpsc` event channel (`sender`, `receiver`).
    *   Spawn `agent_executor.run_operation(...)` in a `tokio::task`, passing the sender.
    *   Listen on the main event receiver for `AgentEvent`s:
        *   Update UI based on message parts/results.
        *   Handle `RequestToolApprovals`:
            *   Clear any previous pending approval state.
            *   Store each `ToolApprovalRequest::responder` (e.g., in a `HashMap<String, oneshot::Sender>`).
            *   Emit `AppEvent::RequestToolApproval` for each tool to the UI.
        *   Handle UI responses (e.g., `AppCommand::HandleToolResponse`):
            *   Look up the corresponding `oneshot::Sender` from the map using the `tool_call_id`.
            *   Send the `ApprovalDecision` back via the sender.
            *   Remove the sender from the map.
    *   Handle the final `Result<ApiMessage>` from the completed task.

## Refactoring `DispatchAgent`

*   The `DispatchAgent` tool will be refactored to use this `AgentExecutor`.
*   Its `run` method will:
    *   Act as a client to `AgentExecutor::run_operation`.
    *   Prepare inputs: fixed model (or configurable), initial prompt message, specific system prompt, a hardcoded list of read-only `ApiToolDefinition`s.
    *   Provide a `tool_executor_callback` that captures and uses a `ToolExecutor::read_only()` instance.
    *   Set `approval_mode` likely to `Automatic` (since tools are known safe).
    *   Create a local event channel to consume messages/results internally.
    *   Await the final result and return the aggregated text output.
*   This removes the duplicated agent loop from `dispatch_agent.rs`.

## Other Considerations

*   **Task Isolation and Communication:** This design achieves task isolation by running each agent operation within its own dedicated `tokio::task`. Communication between the agent task and the main application occurs asynchronously using standard `tokio::sync::mpsc` channels, similar to patterns found in actor systems but without requiring a formal actor framework.
*   **Programmatic Control:** The design supports external programmatic control. An API/protocol layer would interact with the `App` layer, which orchestrates `AgentExecutor` runs and relays events.

## Benefits

*   **Decoupling:** Clear separation of concerns.
*   **Testability:** `AgentExecutor` is more testable in isolation.
*   **Flexibility:** Handles dynamic models, toolsets, and approval modes per-operation.
*   **Responsiveness:** Event-driven approach allows progressive UI updates.
*   **Consistency:** All agent logic (main chat, dispatch agent) uses the same engine.
*   **Maintainability:** Core agent loop logic is centralized.

## TODO

- [x] Create `src/app/agent_executor.rs` with basic structs/enums (`AgentExecutor`, `AgentEvent`, `ApprovalMode`, `RequestToolApprovalsEvent`, `AgentExecutorError`).
- [x] Implement `AgentExecutor::run_operation` loop logic (API call, stream handling, tool call delegation).
- [x] Implement `AgentExecutor::handle_tool_calls` for both `Automatic` and `Interactive` modes.
- [x] Refactor `src/app/mod.rs` (`App` struct) to use `AgentExecutor::run_operation`.
    - [x] Remove direct API call logic.
    - [x] Create `tool_executor_callback` closure.
    - [x] Handle `AgentEvent::RequestToolApprovals` in `app_actor_loop`.
    - [x] Integrate `AgentEvent` handling generally.
    - [x] Adjust `OpContext` usage.
- [x] Refactor `DispatchAgent` tool (`src/tools/dispatch_agent.rs`) to use `AgentExecutor::run_operation`.
- [ ] Refactor remaining commands (`clear`, `compact`, `dispatch`) in `src/app/mod.rs`.
- [ ] Add tests for `AgentExecutor`.
- [ ] Review error handling and event reporting.
- [x] Review interactive approval flow (`ApprovalState` management) for edge cases and robustness. -> *Simplified significantly with oneshot channels.*
- [ ] Consider adding `OpContext` or similar lifecycle management *within* `AgentExecutor::run_operation` if needed, or confirm `App` layer management is sufficient. *(Deferred - current approach seems okay)*
