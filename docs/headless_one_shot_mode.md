# Headless / One-Shot Mode Implementation Plan

> **Status:** Draft – ready for refinement & implementation

This document specifies how to add a **headless (non-interactive) one-shot mode** to the project.  The goal is to let callers run a single conversational cycle – including tool invocations – entirely under program control, without the TUI or any interactive approvals.

---

## 1. Scope & Goals

1. **Batch usage** – Invoke the agent once with an initial prompt (or list of messages) and obtain the assistant's final message plus tool results.
2. **Library API** – Expose an ergonomic Rust function that other crates can call.
3. **CLI flag** – Provide a command-line entry point (`--headless`) that wraps the library API for shell scripts.

Out of scope for this milestone:
* Long-lived interactive kernel (will be tackled separately)
* Complex approval workflows (all tools are assumed pre-approved in one-shot mode)

---

## 2. High-Level Architecture

```mermaid
flowchart LR
  subgraph caller
    direction TB
    A[[Rust crate  /  shell script]]
  end

  A -->|1. run_once(...) / CLI| B(OneShotRunner)
  B -->|2. AgentExecutor::run| C[[LLM API]]
  B -->|3. execute_tool_with_cancellation| D[ToolExecutor]
  C --> B
  D --> B
  B -->|4. RunOnceResult| A
```

* **`OneShotRunner`** orchestrates a _single_ agent loop invocation.
* **`AgentExecutor`** (existing) handles streaming completions + tool calls.
* **`ToolExecutor`** (existing) executes tools; in this mode the callback approves every tool automatically.
* The caller receives a `RunOnceResult` containing:
  * `final_msg: Message` – full structured assistant message
  * `tool_results: Vec<ToolResult>` – audit log of each executed tool

---

## 3. Public Library API (`lib.rs`)

```rust
/// Runs the agent once and waits for the final assistant message.
///
/// * `init_msgs` – seed conversation (system + user or multi-turn)
/// * `model`     – which LLM to use
/// * `cfg`       – LLM config with API keys
/// * `timeout`   – optional wall-clock limit
pub async fn run_once(
    init_msgs: Vec<Message>,
    model: Model,
    cfg: &LlmConfig,
    timeout: Option<Duration>,
) -> anyhow::Result<RunOnceResult>;
```

### `RunOnceResult`
```rust
pub struct RunOnceResult {
    pub final_msg: Message,        // Structured content
    pub tool_results: Vec<ToolResult>,
}
```

The function should **not** expose internal channels or require Tokio knowledge from the caller beyond `await`-ing the Future.

---

## 4. `OneShotRunner` Internals (`src/runners/one_shot_runner.rs`)

| Concern                  | Implementation Notes |
|--------------------------|----------------------|
| System prompt            | Load via `include_str!("../../prompts/system_prompt.md")` |
| Tool approval            | Callback directly executes each tool (no user prompt) |
| Timeout / cancellation   | Create `CancellationToken`; spawn a sleeper to cancel after `timeout` |
| Event handling           | Use a small `mpsc` channel; drain `ToolResultReceived` events for the audit log |
| Error mapping            | Convert `AgentExecutorError::Cancelled` → `anyhow!("timed out")` |

Skeleton (abbreviated):
```rust
pub struct OneShotRunner { /* fields */ }

impl OneShotRunner {
    pub fn new(cfg: &LlmConfig) -> Self { /* … */ }

    pub async fn run(/* args */) -> anyhow::Result<RunOnceResult> {
        // 1. build cancellation token (with optional timeout)
        // 2. create AgentExecutor + event channel
        // 3. build callback: always execute tool
        // 4. call executor.run(...).await?
        // 5. drain event_rx for ToolResult audit
        // 6. return RunOnceResult
    }
}
```

---

## 5. Command-Line Interface

* Extend `Cli` enum in `src/main.rs`:
  ```rust
  #[derive(Subcommand)]
  enum Commands {
      Headless {
          #[arg(long)] model: Option<Model>,
          #[arg(long)] prompt: Option<String>,           // single user message
          #[arg(long)] messages_json: Option<PathBuf>,  // JSON file with Vec<Message>
          #[arg(long)] timeout: Option<u64>,            // seconds
      },
      // existing variants…
  }
  ```
* Handler logic:
  1. Parse input into `Vec<Message>`; if both `prompt` and `messages_json` supplied → error.
  2. Call `run_once`.
  3. Print `RunOnceResult` as pretty JSON.

Example usage:
```bash
cargo run -- headless \
  --model gemini-pro \
  --prompt "Review the diff below …" \
  --timeout 120
```

---

## 6. File / Module Layout

```
src/
  runners/
    mod.rs              // `pub mod one_shot_runner;`
    one_shot_runner.rs  // new code
lib.rs                  // re-exports run_once()
```

`Cargo.toml` stays unchanged (no new external deps expected).

---

## 7. Testing Strategy

* **Unit test** – supply a mock `ApiClient` that echoes back a canned assistant message; assert `run_once` resolves.
* **Integration test** (`tests/headless.rs`) – spin up environment with a fake tool; ensure `tool_results` contains expected entry.
* **CLI smoke test** – run binary with `--prompt "hello" --timeout 5` under `cargo test -- --ignored`.

---

## 8. Task Breakdown

1. **Scaffold modules** – create `src/runners`, add mod + `pub use` in `lib.rs`.
2. **Implement OneShotRunner::new & run**.
3. **Expose `run_once` helper** that internally instantiates `OneShotRunner`.
4. **Add CLI sub-command**.
5. **Write unit & integration tests**.
6. **Update README with usage example**.
7. **Code review & merge**.

---

## 9. Future Work (beyond this milestone)

* Replace "auto-approve every tool" with injectable strategy (e.g. deny-list, pre-approved set).
* Surfacing streaming deltas to the caller (return a `Stream<Item = AgentEvent>` instead of waiting).
* Kernel / long-lived mode over stdin/stdout JSON protocol.

---

### Appendix A – Example JSON Output

```jsonc
{
  "final_msg": {
    "role": "assistant",
    "content_blocks": [
      { "type": "text", "text": "Here is the review…" }
    ],
    "id": "msg_123"  // autogenerated
  },
  "tool_results": [
    {
      "tool_call_id": "call_42",
      "output": "diff --git …",
      "is_error": false
    }
  ]
}
```

