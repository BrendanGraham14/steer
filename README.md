# Coder

Coder is an AI-powered agent and CLI tool that assists with software engineering tasks, written in Rust.

## Features

- Terminal-based chat interface
- Context-aware tooling for file operations, search, and more
- Headless one-shot mode for programmatic and CLI usage
- Git integration
- Memory management via CLAUDE.md files
- API key management from multiple sources
- Conversation history management
- File operation tooling: view, edit, replace, glob search, grep search, etc.

## Usage

```bash
# Start a conversation using API key from .env or environment
coder

# Start a conversation using a specific API key
coder --api-key YOUR_API_KEY

# Start a conversation in a specific directory
coder --directory /path/to/your/project

# Initialize a configuration file
coder init

# Run in headless one-shot mode reading prompt from stdin
echo "What is 2+2?" | coder headless --timeout 30

# Run in headless one-shot mode with a JSON file containing messages
coder headless --messages-json /path/to/messages.json --model gemini-pro

# Clear conversation history
coder clear

# Compact conversation to save context space
coder compact
```

## API Key Setup

Coder loads API keys for different providers. The primary way to provide these keys is through environment variables. These can be set directly in your shell or via a `.env` file in your project's root directory.

The supported environment variables are:

- **Anthropic (Claude):** `ANTHROPIC_API_KEY` (alternatively, `CLAUDE_API_KEY` is also checked)
- **OpenAI:** `OPENAI_API_KEY`
- **Google (Gemini):** `GEMINI_API_KEY`

**Order of Precedence:**

1.  Provider-specific environment variables (e.g., `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`).
2.  Values from a `.env` file (which populates the environment variables).

The configuration file created by `coder init` (`config.json`) is used for storing preferences like the default model and history size, not for API keys directly.

## Commands and Tools

Coder supports a variety of commands and tools to help with pair programming:

- `/help` - Get help with using Coder
- `/model` - View or change the current LLM model
- `/clear` - Clear the current conversation
- `/compact` - Compact the conversation to save context space

## Installation

```bash
# Coming soon
```

## Development

```bash
# Clone the repository
git clone https://github.com/yourusername/coder.git
cd coder

# Build the project
cargo build

# Run the project
cargo run

# Run tests
cargo test
```

## Tool Approval System

Coder includes a tool approval system to ensure safety when executing tools:

- Read-only tools (view, grep, ls, glob, fetch) do not require approval and execute automatically
- Write tools (edit_file, replace_file, bash, etc.) require explicit approval
- When approving a tool, you can use the "always" option to save that tool to your approved list for the current session
- Tools approved with "always" will not prompt for approval again during the same session

## Headless One-Shot Mode

The headless one-shot mode allows for non-interactive, programmatic usage of Coder:

- Run the AI as a single request-response cycle with automatic tool execution
- Perfect for scripting, automation, and API-like usage
- Supports both simple prompts and structured message JSON files as input
- Returns structured JSON with the assistant's message and all tool result details
- Optional timeout setting to limit execution time

### Example JSON Message Format

```json
[
  {
    "role": "user",
    "content": {
      "content": "Analyze the code in the current directory."
    }
  }
]
```

### Example JSON Output

```json
{
  "final_msg": {
    "role": "assistant",
    "content": {
      "content": [
        {
          "type": "text",
          "text": "Here is my analysis..."
        }
      ]
    }
  },
  "tool_results": [
    {
      "tool_call_id": "call_123",
      "output": "src/main.rs\nsrc/lib.rs\n...",
      "is_error": false
    }
  ]
}
```

