# Steer

Steer is an AI coding agent written in rust.

It includes a TUI, supports headless execution, and exposes a gRPC interface for programmatic usage.

---

## Install

### Using Cargo

```bash
cargo install steer
```

### Using Nix

If you have Nix installed, you can run Steer directly:

```bash
# Run steer without installing
nix run github:brendangraham14/steer

# Or install it into your profile
nix profile install github:brendangraham14/steer
```

---

## Quick start

```bash
# Start an interactive chat in the current directory
steer

# Work inside a different directory
steer --directory /path/to/project

# Start with a session configuration file
steer --session-config config.toml

# Point the client at a remote gRPC server instead of running locally
steer --remote 127.0.0.1:50051
```

### Headless mode

```bash
# Read prompt from stdin and return a single JSON response
echo "What is 2+2?" | steer headless

# Provide a JSON file containing `Vec<Message>` in the Steer message format
steer headless --messages-json /tmp/messages.json

# Run inside an existing session (keeps history / tool approvals)
steer headless --session b4e1a7de-2e83-45ad-977c-2c4efdb3d9c6 < prompt.txt

# Supply a custom session configuration (tool approvals, MCP backends, etc.)
steer headless --session-config config.toml < prompt.txt
```

### Authentication

```bash
# Launch Steer and follow the first-run setup wizard
steer

# Re-run the wizard any time inside the chat
/auth
```

### gRPC server / remote mode

```bash
# Start a standalone server (default 127.0.0.1:50051)
steer server --port 50051

# Connect to an already running server
steer tui --remote 192.168.1.10:50051
```

### Session management

```bash
# List saved sessions
steer session list --limit 20

# Delete a session
steer session delete <SESSION_ID> --force

# Create a new session with a config file
steer session create --session-config config.toml

# Create with overrides
steer session create --session-config config.toml --system-prompt "Custom prompt"
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

Steer supports multiple transport types for connecting to MCP servers:

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
/auth        Set up authentication for AI providers
/model       Show or change the current model
/clear       Clear conversation history and tool approvals
/compact     Summarise older messages to save context space
/theme       Change or list available themes
```

---

## Notifications

Steer provides context-aware notifications with different sounds for different events:

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
export STEER_NOTIFICATION_SOUND=false

# Disable desktop notifications
export STEER_NOTIFICATION_DESKTOP=false
```

---

## Authentication

Steer supports multiple methods for providing credentials.

### Interactive Setup

The first time you start Steer it should launch a setup wizard. If it does not, you may trigger it from the chat with the `/auth` command.

All providers (Anthropic, OpenAI, Gemini, xAI) support API key authentication.
 
For Claude Pro/Max users, Steer also supports authenticating via OAuth. **Note**: If OAuth tokens and an API key are saved for Anthropic, the OAuth token takes precedence.

Credentials are stored securely using the OS-native keyring.

### Environment Variables

Steer will detect the following environment variables:

* `ANTHROPIC_API_KEY` or `CLAUDE_API_KEY`
* `OPENAI_API_KEY`
* `GEMINI_API_KEY`
* `XAI_API_KEY` or `GROK_API_KEY` 

Environment variables take precedence over stored credentials.

---

## Tool approval system

Read-only tools run automatically ( `view`, `grep`, `ls`, `glob`, `fetch`, `todo.read` ).  Mutating tools ( `edit`, `replace`, `bash`, etc.) ask for confirmation the first time; choose **always** to remember the decision for the rest of the session.

Headless mode pre-approves every built-in tool for convenience.

### Bash Command Approval

You can pre-approve specific bash commands using glob patterns in your session configuration:

```toml
[tool_config.tools.bash]
approved_patterns = [
    "git status",
    "git log*",
    "npm run*",
    "cargo build*"
]
```
