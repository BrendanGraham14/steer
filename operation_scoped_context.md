# Plan: Implementing Operation-Scoped Context for Cancellation

**Date:** 2024-07-27

## 1. Context

The current implementation attempts to support cancellation of long-running operations (API calls, tool executions) initiated by user input. It utilizes a combination of:

*   `tokio_util::sync::CancellationToken` passed to the `api::Client`.
*   An `Arc<tokio::sync::Notify>` instance (`cancellation_notifier`) raced against API call futures using `tokio::select!`.
*   Manual tracking of spawned tool tasks in `active_tool_tasks: HashMap<String, (JoinHandle<()>, usize)>`.
*   Task abortion via `JoinHandle::abort()` in `cancel_current_processing`.
*   Manual batch tracking (`tool_batches`, `next_batch_id`, `handle_batch_progress`) to manage tool results.

This approach suffers from several drawbacks:
*   **Mixed Signalling:** Uses both `Notify` and `CancellationToken` for signalling, increasing complexity and potential for race conditions or desynchronization.
*   **Global Token Lifetime:** The `CancellationToken` (`current_op_token`) in `App` is created once and never reset, meaning a single cancellation permanently affects subsequent operations.
*   **Lack of Propagation:** Cancellation signals (`Notify`, `CancellationToken`) are not consistently propagated to internally spawned operations like `DispatchAgent` or `CommandFilter`, which create their own independent tokens. Aborting tasks doesn't guarantee immediate cessation of their internal work (like network requests).
*   **Manual Task Management:** Relying on `HashMap`s (`active_tool_tasks`, `tool_batches`) for tracking task lifecycles and results is complex, manual, and error-prone. `JoinHandle::abort()` is less graceful than cooperative cancellation.

## 2. Rationale

To address these issues, we will adopt the **Operation-Scoped Context Object** architecture. This approach involves creating a dedicated context for each user-initiated, potentially long-running operation (e.g., processing a user message).

This architecture was chosen because:
*   **Unified Signalling:** It relies solely on `CancellationToken` for cancellation signalling, removing ambiguity and simplifying logic.
*   **Scoped Lifetime:** Each operation gets a fresh `CancellationToken`, ensuring cancellation is correctly scoped.
*   **Structured Concurrency:** It utilizes `tokio::task::JoinSet` to manage the lifecycle of all asynchronous sub-tasks spawned during an operation, automating joining and simplifying cleanup.
*   **Tokio Idioms:** It leverages standard and well-understood primitives from the Tokio ecosystem.
*   **Reduced Complexity:** It eliminates the need for manual tracking maps like `active_tool_tasks` and potentially `tool_batches`, significantly simplifying state management in `App`.
*   **Improved Robustness:** Promotes cooperative cancellation by passing the token down, which is generally safer than relying solely on `abort()`. **Enables potential future use of hierarchical cancellation (`child_token()`) for finer-grained control, though the current plan uses a single token per operation.**
*   **Manageable Refactoring:** Provides substantial benefits with a moderate refactoring effort compared to more drastic architectural shifts.

## 3. Proposed Solution: `OpContext`

We will introduce an `OpContext` struct, likely within the `app` module:

```rust
use anyhow::Result;
use std::collections::HashMap;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use crate::api::ToolCall; // Assuming ToolCall definition location

// Represents the result of a spawned task within an operation
#[derive(Debug)] // Added Debug for logging/debugging
pub struct TaskResult {
   pub task_id: String, // Typically the tool_call_id, or a synthetic ID for other tasks
   pub result: Result<()>, // Specific success/failure info might be added later if needed
}

// Holds the state for a single, cancellable user-initiated operation
pub struct OpContext {
    pub cancel_token: CancellationToken,
    // Tasks return their ID and Result
    pub tasks: JoinSet<TaskResult>,
    // Store pending approvals within the operation's context
    pub pending_tool_calls: HashMap<String, ToolCall>,
    // Track expected tool results for the current step
    pub expected_tool_results: usize,
    // Track active tools by ID -> tool info
    pub active_tools: HashMap<String, ActiveTool>,
    // Flag indicating whether an API call is in progress
    pub api_call_in_progress: bool,
}

impl OpContext {
    pub fn new() -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            tasks: JoinSet::new(),
            pending_tool_calls: HashMap::new(),
            expected_tool_results: 0,
            active_tools: HashMap::new(),
            api_call_in_progress: true, // Start with API call in progress
        }
    }

    // Set API call status
    pub fn set_api_call_status(&mut self, in_progress: bool) {
        self.api_call_in_progress = in_progress;
    }

    // Adds a tool to the active tools map
    pub fn add_active_tool(&mut self, id: String, name: String) {
        self.active_tools.insert(id.clone(), ActiveTool { id, name });
    }

    // Removes a tool from the active tools map
    pub fn remove_active_tool(&mut self, id: &str) -> Option<ActiveTool> {
        self.active_tools.remove(id)
    }

    // Check if we have any active operations
    pub fn has_activity(&self) -> bool {
        self.api_call_in_progress || !self.active_tools.is_empty() || !self.pending_tool_calls.is_empty()
    }

    // Convenience method to cancel the operation and shut down tasks
    pub async fn cancel_and_shutdown(&mut self) {
        self.cancel_token.cancel();
        // Clear pending calls as they are now irrelevant
        self.pending_tool_calls.clear();
        self.tasks.shutdown().await;
    }
}
```

*   An instance of `OpContext` will be created at the beginning of each operation (e.g., in `App::process_user_message`).
*   The `cancel_token` will be cloned and passed down to all asynchronous functions and sub-tasks related to that operation.
*   All sub-tasks (like tool executions) will be spawned using `op_context.tasks.spawn(...)`, returning a `TaskResult` identifying the task and its outcome.
*   Cancellation will be triggered by calling `op_context.cancel_token.cancel()`.
*   The main operation loop will use `op_context.tasks.join_next().await` to await the completion of sub-tasks and handle their `TaskResult` or join errors, using the ID to identify the task.
*   `op_context.tasks.shutdown().await` ensures all spawned tasks are awaited before the context is dropped, preventing orphaned tasks.
*   Tool calls needing approval will be stored in `op_context.pending_tool_calls`.

## 4. Migration Strategy

We will perform the refactoring in stages to maintain a compilable state:

1.  **Introduce `OpContext`:** Define the struct and `TaskResult`. Add `current_op_context: Option<OpContext>` to `App`. Integrate its creation/clearing only in `App::process_user_message` initially. Keep legacy cancellation paths (`Notify`, old `current_op_token`) for other commands for now. Ensure compilation.
2.  **Shift API Calls:** Update `handle_response` and `check_batch_completion` (or their successors) to use `op_context.cancel_token` and remove the `Notify` logic. Ensure compilation.
3.  **Shift Tool Execution:** Modify `execute_tool_and_handle_result` to accept the token and return `TaskResult`. Start spawning tool tasks via `op_context.tasks.spawn(...)` within a new `join_next` loop structure in `process_user_message`. Begin removing `active_tool_tasks`. Adapt internal API users (`DispatchAgent`, etc.) and the `Tool` trait to accept the token. Ensure compilation.
4.  **Replace Batching:** Fully implement the `join_next` loop to track `expected_tool_results` (stored in `OpContext`). Remove `handle_batch_progress`, `AppEvent::ToolBatchProgress`, and `tool_batches`/`next_batch_id` once **all tool execution paths are confirmed to use the `OpContext`/`JoinSet` mechanism and no longer rely on the batching system (verify via code review and testing)**. Ensure compilation.
5.  **Refactor Approval:** Update `handle_tool_command_response` to interact with `op_context.pending_tool_calls` and spawn approved tasks into `op_context.tasks`. Remove the old `pending_tool_calls` map from `App`. Ensure compilation.
6.  **Introduce UI Events & Update TUI:** Add new `AppEvent` variant `OperationCancelled` with clear information about what was cancelled. Update spawning/joining logic to emit the appropriate event. Modify the TUI to consume these events. Ensure compilation and correct UI feedback.
7.  **Audit Other Operations:** Analyze and refactor `/compact`, `/dispatch`, etc., if needed for cancellation consistency. Ensure compilation.
8.  **Cleanup:** Remove any remaining legacy cancellation fields (`cancellation_notifier`, old `current_op_token`) and logic.

## 5. Detailed Implementation Steps

1.  **Define `OpContext` & `TaskResult`:** (Covered in Section 3 and Migration Strategy)
    *   Create the `OpContext` and `TaskResult` structs as defined above (e.g., in `src/app/context.rs` and `pub use` in `src/app/mod.rs`).

2.  **Integrate `OpContext` into `App`:** (Covered in Migration Strategy)
    *   Add `current_op_context: Option<OpContext>`.
    *   *Initially keep* `tool_batches`, `next_batch_id` during migration. Mark for removal later.
    *   Remove `cancellation_notifier`.
    *   Remove `active_tool_tasks`.
    *   Remove the top-level `pending_tool_calls` (it moves into `OpContext`).

3.  **Manage `OpContext` Lifecycle:** (Covered in Migration Strategy)
    *   In `App::process_user_message` (and potentially others):
        *   Handle pre-existing `current_op_context` (cancel or error).
        *   Create and store `OpContext`.
        *   Implement the main `join_next` loop (see step 6 details).
        *   Ensure `current_op_context` is cleared on completion/error/cancellation.
    *   Modify `App::cancel_current_processing`:
        *   Call `ctx.cancel_token.cancel()` if `current_op_context` exists. The `join_next` loop handles shutdown.

4.  **Update API Call Handling:** (Covered in Migration Strategy)
    *   Modify `App::handle_response`, `App::check_batch_completion` (or successors):
        *   Remove `select!` against `notifier.notified()`.
        *   Pass `op_context.cancel_token.clone()` to `get_claude_response`.
        *   Handle `"Request cancelled"` errors.

5.  **Refactor Tool Execution & Propagation:**
    *   Modify `execute_tool_and_handle_result`:
        *   Remove `batch_id`, `internal_event_sender` params.
        *   Add `token: CancellationToken`, `tool_call_id: String`, `tool_name: String` params.
        *   Return `Result<()>`.
        *   Add `select!` around `tool_executor.execute_tool` and other long operations internally if needed.
    *   Modify `ToolExecutor::execute_tool` **and the underlying `Tool` trait's execution method (if applicable)** to accept `token: CancellationToken`. Ensure individual tool implementations can cooperatively cancel if necessary.
    *   Modify internal API users (`CommandFilter::evaluate_command_safety`, `DispatchAgent::execute`, `Conversation::compact`) to accept `token: CancellationToken` and pass it to `api_client.complete`. Update call sites.
    *   In `App`, spawn tasks using `op_context.tasks.spawn(...)`, wrapping the call to `execute_tool_and_handle_result` to capture the ID and result: `op_context.tasks.spawn(async move { let res = execute_tool(..., token.clone()).await; TaskResult { task_id: tool_call_id.clone(), result: res } })`. **Emit `AppEvent::ToolExecutionStarted` here.**
    *   **Note on Blocking Work:** Identify any significant blocking I/O or CPU-bound work. **Areas to audit include:** file system operations (e.g., within `file_search`, `read_file`, `edit_file` tools if they handle large files synchronously), external processes (`run_terminal_cmd`), potentially `grep_search` implementation. These should either be run via `tokio::task::spawn_blocking` (checking the token periodically inside the blocking closure if possible) or refactored to be async with periodic `token.is_cancelled()` checks.

6.  **Implement `join_next` Loop & Replace Batching:** (Covered in Migration Strategy)
    *   Remove `App::handle_batch_progress`, `AppEvent::ToolBatchProgress`.
    *   The main operation loop (likely within `process_user_message` or a dedicated helper function called by it) replaces batching.
    *   Store the number of tools expected from the initial API response in `op_context.expected_tool_results`.
    *   Inside the loop:
        *   Use `tokio::select!` to race `op_context.tasks.join_next()` against `op_context.cancel_token.cancelled()`.
        *   If `join_next()` yields `Some(join_result)`:
            *   Handle `Ok(task_result)`:
                *   Log success/failure based on `task_result.result`.
                *   **Emit `AppEvent::ToolExecutionCompleted { id: task_result.task_id, ... }`.**
                *   Decrement `op_context.expected_tool_results`.
                *   Check if `op_context.expected_tool_results == 0` **and** `op_context.pending_tool_calls.is_empty()`.
                *   If ready, call `continue_operation_after_tools(&mut op_context)` (new helper function) which makes the next API call, potentially gets new tools, and updates `op_context.expected_tool_results` and `op_context.pending_tool_calls`.
            *   Handle `Err(join_err)` (panic): Log error, consider cancelling (`op_context.cancel_token.cancel()`).
        *   If `join_next()` yields `None` (JoinSet empty or shutdown): Break the loop.
        *   If `cancel_token.cancelled()` fires: Break the loop.
    *   After the loop, ensure `self.current_op_context` is set to `None`. For normal completion, emit `AppEvent::ThinkingCompleted`; for cancellation, emit the appropriate `AppEvent::OperationCancelled` event.

7.  **Tool Approval Flow:** (Covered in Migration Strategy)
    *   When initiating calls (`initiate_tool_calls` replacement): Populate `op_context.pending_tool_calls` for tools needing approval. Don't spawn them yet. Set `op_context.expected_tool_results` only for *approved* tools being spawned immediately.
    *   Modify `App::handle_tool_command_response`:
        *   Get mutable access to context: `let op_context = self.current_op_context.as_mut().ok_or_else(|| anyhow!("No active operation for tool response"))?;`
        *   If approved, remove from `op_context.pending_tool_calls`, spawn into `op_context.tasks` (remembering to emit `ToolExecutionStarted`), and increment `op_context.expected_tool_results`.
        *   If denied, remove from `op_context.pending_tool_calls`, add a "Denied" result to the conversation. Check if `op_context.expected_tool_results == 0` and `op_context.pending_tool_calls.is_empty()` to potentially call `continue_operation_after_tools`.

8.  **Testing:**
    *   **Test Scenarios:** Verify cancellation and correct behavior in situations including:
        *   Cancellation during initial API request (before tools).
        *   Cancellation while multiple tool tasks are running concurrently (some finished, some not).
        *   Cancellation while tools are pending approval.
        *   Correct operation flow when one tool is approved and another is denied.
        *   Rapid successive user messages correctly cancelling the previous operation and starting the new one.
        *   Successful completion of multi-step tool use involving intermediate API calls.
        *   Error handling when a tool task panics or returns an error.
        *   Correct UI feedback (spinners starting/stopping) based on `ToolExecutionStarted`/`Completed` events.
