# Claude Code RS

A command line tool for pair programming with Claude, written in Rust.

## Features

- Terminal-based chat interface with Claude
- Context-aware tooling for file operations, search, and more
- Git integration
- Memory management via CLAUDE.md files
- API key management from multiple sources
- Conversation history management
- File operation tooling: view, edit, replace, glob search, grep search, etc.

## Usage

```bash
# Start a conversation with Claude using API key from .env or environment
claude-code-rs

# Start a conversation with Claude using a specific API key
claude-code-rs --api-key YOUR_API_KEY

# Start a conversation in a specific directory
claude-code-rs --directory /path/to/your/project

# Initialize a configuration file
claude-code-rs init

# Clear conversation history
claude-code-rs clear

# Compact conversation to save context space
claude-code-rs compact
```

## API Key Setup

You can provide your Claude API key in several ways (in order of precedence):

1. Command line argument: `--api-key YOUR_API_KEY` or `-a YOUR_API_KEY`
2. Environment variable: `CLAUDE_API_KEY=YOUR_API_KEY`
3. `.env` file in your current directory: `CLAUDE_API_KEY=YOUR_API_KEY`
4. Config file (created with `claude-code-rs init`)

## Commands and Tools

Claude Code RS supports a variety of commands and tools to help with pair programming:

- `/help` - Get help with using Claude Code

## Installation

```bash
# Coming soon
```

## Development

```bash
# Clone the repository
git clone https://github.com/yourusername/claude-code-rs.git
cd claude-code-rs

# Build the project
cargo build

# Run the project
cargo run

# Run tests
cargo test
```

## License

MIT
