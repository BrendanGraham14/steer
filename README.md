# Conductor

Conductor is an AI-powered CLI assistant for software engineering tasks. It provides an interactive terminal chat UI, a fully automated headless mode, and a gRPC server that lets other processes talk to the agent.

---

## Quick start

```bash
# Start an interactive chat in the current directory
conductor

# Work inside a different directory
conductor --directory /path/to/project

# Use a specific model (run `conductor /model` at runtime to list models)
conductor --model opus

# Point the client at a remote gRPC server instead of running locally
conductor --remote 127.0.0.1:50051
```

### Headless one-shot mode

```bash
# Read prompt from stdin and return a single JSON response
echo "What is 2+2?" | conductor headless

# Provide a JSON file containing `Vec<Message>` in the Conductor message format
conductor headless --messages-json /tmp/messages.json --model gemini-pro

# Run inside an existing session (keeps history / tool approvals)
conductor headless --session b4e1a7de-2e83-45ad-977c-2c4efdb3d9c6 < prompt.txt

# Supply a custom tool-visibility / pre-approval configuration
conductor headless --tool-config tools.json < prompt.txt
```

### gRPC server / remote mode

```bash
# Start a standalone server (default 127.0.0.1:50051)
conductor server --port 50051

# Connect to an already running server
conductor tui --remote 192.168.1.10:50051
```

### Session management

```bash
# List saved sessions
conductor session list --limit 20

# Delete a session
conductor session delete <SESSION_ID> --force
```

---

## Slash commands (inside the chat UI)

```
/help        Show help
/model       Show or change the current model
/clear       Clear conversation history and tool approvals
/compact     Summarise older messages to save context space
/cancel      Cancel the current operation in progress
```

---

## API keys

Conductor only looks at environment variables (optionally loaded from a `.env` file).  Set the variables that correspond to the providers you intend to use:

* `ANTHROPIC_API_KEY` or `CLAUDE_API_KEY`
* `OPENAI_API_KEY`
* `GEMINI_API_KEY`

The `conductor init` command creates `~/.config/conductor/config.json` for preferences such as default model or history size, **not** for secrets.

---

## Tool approval system

Read-only tools run automatically ( `view`, `grep`, `ls`, `glob`, `fetch`, `todo.read` ).  Mutating tools ( `edit`, `replace`, `bash`, etc.) ask for confirmation the first time; choose **always** to remember the decision for the rest of the session.

Headless mode pre-approves every built-in tool for convenience.

---

## Development

```bash
git clone https://github.com/yourusername/conductor.git
cd conductor

# Build & test
cargo build
cargo test
```

Run `cargo run -p conductor-cli` to launch the CLI in the current directory.

---

## Project layout (crates)

* `conductor-proto`     – Protocol buffer definitions and generated code
* `conductor-core`      – Pure domain logic (LLM APIs, session management, tool execution)
* `conductor-grpc`      – gRPC server/client implementation and core ↔ proto conversions
* `conductor-tui`       – Terminal UI library (ratatui-based)
* `conductor-cli`       – Command-line interface and main binary
* `tools`               – Tool trait definitions and implementations
* `macros`              – Procedural macros for tool definitions
* `remote-backend`      – gRPC service for remote tool execution

### Architecture principles

1. **Clean dependency graph**: proto → core → grpc → cli/tui
2. **Single API surface**: All clients (TUI, headless, etc.) go through gRPC
3. **No in-process shortcuts**: Even local mode uses an in-memory gRPC channel
4. **Clear boundaries**: Each crate has a single, well-defined responsibility

Full details live in `ARCHITECTURE.md`.
