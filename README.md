# Steer

Steer is an AI coding agent written in Rust.

It includes a TUI, supports headless execution, and exposes a gRPC interface for programmatic usage.

https://github.com/user-attachments/assets/5a31ccc9-6a96-4005-ab5a-ae3aa7ae34c4

---

## Install

### Shell Script
```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/BrendanGraham14/steer/releases/latest/download/steer-installer.sh | sh
```

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
  workspace    Workspace management commands
  help         Print this message or the help of the given subcommand(s)

Options:
      --session <SESSION>
          Resume an existing session instead of starting a new one (local or remote modes)
  -d, --directory <DIRECTORY>
          Optional directory to work in
  -m, --model <MODEL>
          Model to use (e.g., 'codex', 'opus', 'sonnet', 'gemini', 'grok', 'openai/custom-model')
      --remote <REMOTE>
          Connect to a remote gRPC server instead of running locally
      --session-config <SESSION_CONFIG>
          Path to session configuration file (TOML format) for new sessions
      --theme <THEME>
          Theme to use for the TUI (falls back to "catppuccin-mocha" when unset)
      --catalog <PATH>
          Additional catalog files to load (repeatable)
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

### Config Files

Steer reads config from two places, the project-level `.steer/` directory in your current working directory and the following user-level directory:

| Platform | User-level config directory              |
| -------- | ---------------------------------------- |
| macOS    | `~/Library/Application Support/steer/`   |
| Linux    | `~/.config/steer/`                       |
| Windows  | `%APPDATA%\steer\`                       |

Both config directories can contain the same two config files:

- `catalog.toml` - Defines model providers and models. Steer always includes a built-in catalog, auto-discovers project/user catalogs, and accepts additional `--catalog <PATH>` files (repeatable). Later catalogs override earlier ones.
- `session.toml` - Defines defaults for new sessions. Auto-discovery order is project first, then user config, and first existing file wins. Override discovery with `--session-config <PATH>`.

### gRPC server / remote mode

You can supply one or more catalogs with `--catalog`.

- Server: catalogs passed to `steer server` are loaded on the server and define available providers/models.
- Local TUI: passing `--catalog` affects the in-process server started by the CLI.

```bash
# Start a standalone server (default 127.0.0.1:50051) with additional catalogs
steer server --port 50051 --catalog ./my-catalog.toml

# Local TUI using a custom catalog (applies to the local in-process server)
steer --catalog ./my-catalog.toml
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

# Resume a session
steer --session <SESSION_ID>
```

### Workspaces

Workspaces track the working directory and VCS state for sessions.

```bash
# List all workspaces
steer workspace list

# Show workspace status (VCS info, path, etc.)
steer workspace status --workspace-id <WORKSPACE_ID>
steer workspace status --session-id <SESSION_ID>
# equivalent using the global --session flag
steer --session <SESSION_ID> workspace status
```

### Session Configuration Files

Sessions can be configured using TOML configuration files. This is useful for:
- Consistent project-specific configurations
- Setting up MCP (Model Context Protocol) backends
- Pre-approving tools or bash commands
- Sharing configurations with your team

> **Note:** `system_prompt` is not supported in session config files. System prompts are configured via agent modes (see [Agent Modes](#agent-modes)).

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
[tool_config]
visibility = "all"

[tool_config.approvals]
tools = ["grep", "ls", "read_file", "glob", "read_todos", "write_todos"]

[tool_config.approvals.bash]
patterns = ["git status", "git log*", "npm run*", "cargo build*"]
```

#### Example: Full Configuration
```toml
# session-full.toml

[workspace]
type = "local"

[tool_config]
backends = [
  { type = "mcp", server_name = "calculator", transport = { type = "stdio", command = "python", args = ["-m", "mcp_calculator"] }, tool_filter = "all" },
  { type = "mcp", server_name = "web-tools", transport = { type = "tcp", host = "127.0.0.1", port = 3000 }, tool_filter = "all" }
]
visibility = "all"

[tool_config.approvals]
tools = ["grep", "ls", "read_file", "glob", "read_todos", "write_todos"]

[tool_config.approvals.bash]
patterns = ["git status", "git diff", "git log*", "npm test", "cargo check"]

[tool_config.approvals.dispatch_agent]
agent_patterns = ["explore"]

[auto_compaction]
enabled = true            # default: true
threshold_percent = 90    # default: 90

[metadata]
project = "my-project"
environment = "development"
```

#### Auto-Compaction

Steer automatically compacts conversations when the context window fills up. This is configurable in session config:

```toml
[auto_compaction]
enabled = true            # default: true
threshold_percent = 90    # trigger compaction at 90% context usage (default: 90)
```

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

#### HTTP
```toml
transport = { type = "http", url = "http://localhost:3000", headers = { "X-API-Key" = "secret" } }
```

---

## Slash commands (inside the chat UI)

```
/help           Show help
/auth           Set up authentication for AI providers
/model          Show or change the current model
/agent          Show or switch primary agent mode (normal/plan/yolo) [alias: /mode]
/compact        Summarize the current conversation
/new            Start a new conversation session
/theme          Change or list available themes
/mcp            Show MCP server connection status
/workspace      Show workspace status
/editing-mode   Switch between simple and vim editing modes
/reload-files   Reload file cache
```

---

## Agent Modes

Steer has three primary agent modes that control tool visibility and approval behavior. Switch modes at any time with `/agent` (or `/mode`).

| Mode | Description |
|------|-------------|
| **normal** (default) | Full tool visibility. Mutating tools (bash, edit, etc.) require explicit approval. |
| **plan** | Read-only tools only (plus `dispatch_agent` for exploration). Use this mode to research and plan before making changes. |
| **yolo** | Full tool visibility with auto-approval for all tools. No manual approval prompts. |

```
# Show current mode
/agent

# Switch to plan mode
/agent plan

# Switch to yolo mode
/agent yolo
```

---

## Custom Commands

Define project-specific slash commands in `.steer/commands.toml`:

```toml
[[commands]]
type = "prompt"
name = "review"
description = "Careful and thorough change review"
prompt = """
Review the diff from `jj diff -r main..@` carefully.

1. **Understand the change.** What is this change doing and why?
2. **Architecture & design.** Are there structural problems? Is the high-level approach sound?
3. **Simplification.** Can the design or implementation be simpler?
4. **Correctness & completeness.** Are there bugs, edge cases, or missing pieces?

Be specific — reference files and line numbers.
"""
```

Custom commands appear as slash commands in the TUI (e.g., `/review`).

---

## Notifications

Steer supports terminal notifications when certain events occur.

### Notification Types

| Notification Type | When |
|---|---|
| **Processing Complete** | Assistant finishes processing your request |
| **Tool Approval** | A tool needs your approval (e.g., `bash`, `edit_file`) |
| **Error** | An error occurs during processing |

### Configuration

Notifications are configured via `steer preferences edit`:

```toml
[ui.notifications]
transport = "auto" # auto | osc9 | off
```

- `transport = "auto"` (default) uses OSC 9 terminal notifications.
- In terminals like Ghostty, notification clicks can switch back to the relevant tab.

---

## Authentication

Steer supports multiple methods for providing credentials.

### Interactive Setup

The first time you start Steer it should launch a setup wizard. The auth flow can also be triggered via the `/auth` command.

All providers (Anthropic, OpenAI, Google, xAI) support API key authentication.

Steer also supports authenticating via OAuth for:
- **Anthropic** — Claude Pro/Max users
- **OpenAI** — ChatGPT Plus/Pro/Team users (Codex)

**Note**: If OAuth tokens and an API key are both saved for a provider, the OAuth token takes precedence.

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

Steer provides a set of built-in tools that the AI agent uses to interact with your codebase.

### Built-in Tools

**Auto-approved by default** (no approval prompt in normal mode):

| Tool | Description |
|------|-------------|
| `grep` | Fast content search (ripgrep-based, regex support) |
| `astgrep` | Structural code search using AST patterns |
| `glob` | File pattern matching (e.g., `**/*.js`) |
| `ls` | List files and directories |
| `read_file` | Read file contents |
| `read_todos` | Read session to-do list |
| `write_todos` | Update the session to-do list |

`write_todos` mutates only the in-session to-do list and is intentionally auto-approved.

**Mutating** (require approval on first use):

| Tool | Description |
|------|-------------|
| `bash` | Run shell commands |
| `edit_file` | Edit a file (find-and-replace) |
| `multi_edit` | Apply multiple edits to one file in a single call |
| `write_file` | Write/overwrite entire file contents |
| `web_fetch` | Fetch and process web content |
| `dispatch_agent` | Launch sub-agents for focused tasks |

### Pre-approving Tools

You can pre-approve specific tools and bash command patterns in your session configuration:

```toml
# Pre-approve entire tools by name
[tool_config.approvals]
tools = ["grep", "ls", "read_file", "glob", "read_todos", "write_todos"]

# Pre-approve specific bash commands using glob patterns
[tool_config.approvals.bash]
patterns = [
    "git status",
    "git log*",
    "npm run*",
    "cargo build*"
]

# Pre-approve specific dispatch_agent sub-agents
[tool_config.approvals.dispatch_agent]
agent_patterns = ["explore"]
```

---

## Preferences

User preferences are stored in `~/.config/steer/preferences.toml` (macOS/Linux). Manage them with:

```bash
steer preferences show
steer preferences edit
steer preferences reset
```

### Available Preferences

```toml
# Default model to use
default_model = "codex"

[ui]
theme = "catppuccin-mocha"
editing_mode = "simple"     # simple | vim
history_limit = 100         # conversation history limit
provider_priority = ["anthropic", "openai", "google", "xai"]

[ui.notifications]
transport = "auto"          # auto | osc9 | off

[tools]
pre_approved = []           # tools to pre-approve globally

[telemetry]
enabled = true
```

### Vim Editing Mode

Steer supports vim keybindings in the input editor. Enable via:

```
/editing-mode vim
```

Or set permanently in preferences:

```toml
[ui]
editing_mode = "vim"
```
