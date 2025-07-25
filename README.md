# Steer

Steer is an AI coding agent written in rust.

It includes a TUI, supports headless execution, and exposes a gRPC interface for programmatic usage.

---

## Install

### Cargo

```bash
cargo install steer
```

## Quick Start

Simply run `steer` to start the TUI in a local session.

More options:

```
❯ steer --help
Command-line interface for Steer coding agent.

Usage: steer [OPTIONS] [COMMAND]

Commands:
  tui          Launch the interactive terminal UI (default)
  preferences  Manage user preferences
  headless     Run in headless mode
  server       Start the gRPC server
  session      Session management commands
  help         Print this message or the help of the given subcommand(s)

Options:
      --session <SESSION>
          Resume an existing session instead of starting a new one (local or remote modes)
  -d, --directory <DIRECTORY>
          Optional directory to work in
  -m, --model <MODEL>
          Model to use [default: claude-opus-4-20250514] [possible values: claude-3-5-sonnet-20240620, claude-3-5-sonnet-20241022, claude-3-7-sonnet-20250219, claude-3-5-haiku-20241022, claude-sonnet-4-20250514, claude-opus-4-20250514, gpt-4.1-2025-04-14, gpt-4.1-mini-2025-04-14, gpt-4.1-nano-2025-04-14, o3-2025-04-16, o3-pro-2025-06-10, o4-mini-2025-04-16, gemini-2.5-flash-preview-04-17, gemini-2.5-pro-preview-05-06, gemini-2.5-pro-preview-06-05, grok-3, grok-3-mini, grok-4-0709]
      --remote <REMOTE>
          Connect to a remote gRPC server instead of running locally
      --system-prompt <SYSTEM_PROMPT>
          Custom system prompt to use instead of the default
      --session-config <SESSION_CONFIG>
          Path to session configuration file (TOML format) for new sessions
      --theme <THEME>
          Theme to use for the TUI (defaults to "default")
  -h, --help
          Print help
  -V, --version
          Print version
```

### Headless mode

```bash
# Read prompt from stdin and return a single JSON response
echo "What is 2+2?" | steer headless

# Provide a JSON file containing `Vec<Message>` in the Steer message format
steer headless --messages-json /tmp/messages.json

# Run inside an existing session (keeps history / tool approvals), pipe the prompt from a file
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

### Sessions

Steer persists data to a session. You may create, list, delete, and resume sessions.

```bash
# List saved sessions
steer session list --limit 20

# Delete a session
steer session delete <SESSION_ID> --force

# Create a new session with a config file
steer session create --session-config config.toml

# Create with overrides
steer session create --session-config config.toml --system-prompt "Custom prompt"

# Resume a session
steer --session <SESSION_ID>
```

### Session Configuration Files

Sessions can be configured using TOML configuration files. This is useful for:
- Consistent project-specific configurations
- Setting up MCP (Model Context Protocol) backends
- Pre-approving tools or bash commands
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

#### Stdio
```toml
transport = { type = "stdio", command = "python", args = ["-m", "mcp_server"] }
```

#### TCP
```toml
transport = { type = "tcp", host = "127.0.0.1", port = 3000 }
```

#### Unix Socket
```toml
transport = { type = "unix", path = "/tmp/mcp.sock" }
```

#### SSE
```toml
transport = { type = "sse", url = "http://localhost:3000/events", headers = { "Authorization" = "Bearer token" } }
```

#### HTTP
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
/compact     Summarize the current conversation
/theme       Change or list available themes
/mcp         Show MCP server connection status
```

---

## Notifications

Steer supports desktop and audio notifications when certain events occur.

### Notification Types

| Notification Type | When |
|---|---|
| **Processing Complete** | Assistant finishes processing your request |
| **Tool Approval** | A tool needs your approval (e.g., `bash`, `edit_file`) |
| **Error** | An error occurs during processing |

### Configuration

Both sound and desktop notifications are enabled by default. To disable either of them, edit the configuration via `steer preferences edit`:

```toml
[ui.notifications]
sound = true
desktop = true
```

---

## Authentication

Steer supports multiple methods for providing credentials.

### Interactive Setup

The first time you start Steer it should launch a setup wizard. The auth flow can also be triggered via the `/auth` command.

All providers (Anthropic, OpenAI, Gemini, xAI) support API key authentication.
 
For Claude Pro/Max users, Steer also supports authenticating via OAuth. **Note**: If OAuth tokens and an API key are saved for Anthropic, the OAuth token takes precedence.

Credentials are stored securely using the OS-native keyring.

### Environment Variables

Steer can also load credentials via environment variables. It will detect the following environment variables:

* `ANTHROPIC_API_KEY` or `CLAUDE_API_KEY`
* `OPENAI_API_KEY`
* `GOOGLE_API_KEY` or `GEMINI_API_KEY`
* `XAI_API_KEY` or `GROK_API_KEY` 

Environment variables take precedence over stored credentials.

---

## Tool approval system

Read-only tools run automatically ( `view`, `grep`, `ls`, `glob`, `fetch`, `todo.read` ).  Mutating tools ( `edit`, `replace`, `bash`, etc.) require approval on first use, with the option to remember the decision for the rest of the session.


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
