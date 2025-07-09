# Conductor

Conductor is an AI-powered CLI assistant for software engineering tasks. It provides an interactive terminal chat UI, a fully automated headless mode, and a gRPC server that lets other processes talk to the agent.

> *AI guides your code*  
> *Terminal wisdom flows free*  
> *Tasks complete with ease*

---

## Installation

### Using Cargo

```bash
cargo install --git ssh://git@github.com/brendangraham14/conductor conductor-cli --locked
```

### Using Nix

If you have Nix installed, you can run Conductor directly:

```bash
# Run conductor without installing
nix run github:brendangraham14/conductor

# Or install it into your profile
nix profile install github:brendangraham14/conductor
```

### Prerequisites

- **Nix (Optional)**: For Nix-based development, [Nix](https://nixos.org/download.html) (version 2.18 or newer) is required.
- **Direnv (Optional)**: For automatic shell environment loading with Nix, [direnv](https://direnv.net/docs/installation.html) is recommended.
- **macOS**: Xcode Command Line Tools are required for some build dependencies.

---

## Quick start

```bash
# Start an interactive chat in the current directory
conductor

# Work inside a different directory
conductor --directory /path/to/project

# Use a specific model (run `conductor /model` at runtime to list models)
conductor --model opus

# Start with a session configuration file
conductor --session-config session.toml

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

# Supply a custom session configuration (tool approvals, MCP backends, etc.)
conductor headless --session-config session.toml < prompt.txt
```

### Authentication

```bash
# Login to Anthropic (Claude) using OAuth - available for Claude Pro users!
conductor auth login anthropic

# Check authentication status for all providers
conductor auth status

# Logout when done
conductor auth logout anthropic
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

# Create a new session with a config file
conductor session create --session-config session.toml

# Create with overrides
conductor session create --session-config session.toml --system-prompt "Custom prompt"
```

### Session Configuration Files

You can create sessions using TOML configuration files. This is useful for:
- Consistent project-specific configurations
- Setting up MCP (Model Context Protocol) backends
- Pre-approving tools for automation
- Sharing configurations with your team

#### Example: Minimal Configuration
```toml
# session-minimal.toml
[tool_config]
backends = [
  { type = "mcp", server_name = "calculator", transport = { type = "stdio", command = "python", args = ["-m", "mcp_calculator"] }, tool_filter = "all" }
]
```

#### Example: Pre-approved Tools
```toml
# session-preapproved.toml
system_prompt = "You are a helpful coding assistant."

[tool_config]
visibility = "all"
approval_policy = { type = "pre_approved", tools = ["grep", "ls", "view", "glob", "todo_read"] }
```

#### Example: Full Configuration
```toml
# session-full.toml
system_prompt = "You are a helpful assistant with access to calculator and web tools."

[workspace]
type = "local"

[tool_config]
backends = [
  { type = "mcp", server_name = "calculator", transport = { type = "stdio", command = "python", args = ["-m", "mcp_calculator"] }, tool_filter = "all" },
  { type = "mcp", server_name = "web-tools", transport = { type = "tcp", host = "127.0.0.1", port = 3000 }, tool_filter = "all" }
]
visibility = "all"
approval_policy = "always_ask"

[metadata]
project = "my-project"
environment = "development"
```

See the `examples/` directory for more configuration examples.

### MCP Transport Options

Conductor supports multiple transport types for connecting to MCP servers:

#### Stdio Transport (Default)
For MCP servers that communicate via standard input/output:
```toml
transport = { type = "stdio", command = "python", args = ["-m", "mcp_server"] }
```

#### TCP Transport
For MCP servers listening on a TCP port:
```toml
transport = { type = "tcp", host = "127.0.0.1", port = 3000 }
```

#### Unix Socket Transport
For MCP servers using Unix domain sockets (Unix/macOS only):
```toml
transport = { type = "unix", path = "/tmp/mcp.sock" }
```

#### SSE Transport
For MCP servers using Server-Sent Events:
```toml
transport = { type = "sse", url = "http://localhost:3000/events", headers = { "Authorization" = "Bearer token" } }
```

#### HTTP Transport
For MCP servers using streamable HTTP:
```toml
transport = { type = "http", url = "http://localhost:3000", headers = { "X-API-Key" = "secret" } }
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

## Notifications

Conductor provides context-aware notifications with different sounds for different events:

### Notification Types

1. **Processing Complete** ✅
   - Sound: Pleasant completion tone (Glass on macOS, message-new-instant on Linux)
   - When: Assistant finishes processing your request

2. **Tool Approval** ⚠️
   - Sound: Attention-getting alert (Ping on macOS, dialog-warning on Linux)
   - When: A tool needs your approval (e.g., `bash`, `edit_file`)

3. **Error** ❌
   - Sound: Error tone (Basso on macOS, dialog-error on Linux)
   - When: An error occurs during processing

### Configuration

Both sound and desktop notifications are enabled by default. To disable:

```bash
# Disable sound notifications
export CONDUCTOR_NOTIFICATION_SOUND=false

# Disable desktop notifications
export CONDUCTOR_NOTIFICATION_DESKTOP=false
```

---

## Authentication

### OAuth Authentication (Anthropic/Claude)

Conductor supports OAuth authentication for Anthropic's Claude models, allowing Claude Pro subscribers to authenticate without managing API keys:

```bash
# Login to Anthropic using OAuth
conductor auth login anthropic

# Check authentication status
conductor auth status

# Logout from Anthropic
conductor auth logout anthropic
```

When you run `conductor auth login anthropic`:
1. Your browser will open to authorize Conductor
2. After authorizing, you'll see a code on the redirect page
3. Copy the ENTIRE code (including the part after #) and paste it into the terminal
4. Conductor will exchange this for access tokens and store them securely

Tokens are stored securely using:
- **macOS**: Keychain
- **Windows**: Windows Credential Store
- **Linux**: Secret Service API (or encrypted file fallback)

### API Keys (Traditional)

You can still use traditional API keys by setting environment variables (optionally loaded from a `.env` file):

* `ANTHROPIC_API_KEY` or `CLAUDE_API_KEY`
* `OPENAI_API_KEY`
* `GEMINI_API_KEY`
* `GROK_API_KEY`

**Note**: If both OAuth tokens and API keys are available for Anthropic, the API key takes precedence.

The `conductor init` command creates `~/.config/conductor/config.json` for preferences such as default model or history size, **not** for secrets.

---

## Tool approval system

Read-only tools run automatically ( `view`, `grep`, `ls`, `glob`, `fetch`, `todo.read` ).  Mutating tools ( `edit`, `replace`, `bash`, etc.) ask for confirmation the first time; choose **always** to remember the decision for the rest of the session.

Headless mode pre-approves every built-in tool for convenience.

---

## Development

The recommended way to build and test is with Nix, which provides a reproducible environment with all dependencies.

```bash
# Enter the development shell
nix develop

# Or with direnv for automatic environment loading
direnv allow

# Run checks and tests
nix flake check

# Build the project
nix build
```

If you don't have Nix, you can use `cargo` directly, but you'll need to install dependencies like `protobuf` and `pkg-config` manually.

```bash
# Build & test with Cargo
cargo build
cargo test
```

The `justfile` provides helpful commands for common tasks. Run `just` to see the available commands.

---

## Project layout (crates)

* `crates/conductor-proto`     – Protocol buffer definitions and generated code
* `crates/conductor-core`      – Pure domain logic (LLM APIs, session management, tool execution)
* `crates/conductor-grpc`      – gRPC server/client implementation and core ↔ proto conversions
* `crates/conductor-tui`       – Terminal UI library (ratatui-based)
* `crates/conductor-cli`       – Command-line interface and main binary
* `crates/conductor-tools`     – Tool trait definitions and implementations
* `crates/conductor-macros`    – Procedural macros for tool definitions
* `crates/conductor-remote-workspace` – gRPC service for remote tool execution

### Architecture principles

1. **Clean dependency graph**: proto → core → grpc → cli/tui
2. **Single API surface**: All clients (TUI, headless, etc.) go through gRPC
3. **No in-process shortcuts**: Even local mode uses an in-memory gRPC channel
4. **Clear boundaries**: Each crate has a single, well-defined responsibility

Full details live in `ARCHITECTURE.md`.
