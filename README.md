# Claude Code RS

A command line tool to pair program with Claude, inspired by Claude Code.

## Overview

Claude Code RS is a Rust implementation of a terminal-based chat interface for interacting with Claude AI to help with programming tasks. It provides similar functionality to Claude Code but in a standalone command line tool.

> **Note**: This project is currently in development and not yet ready for use. See the Project Status section for details.

## Features (Planned)

- Terminal-based chat interface with Claude
- Support for executing various tools:
  - Bash: Execute shell commands
  - GlobTool: Find files by glob pattern
  - GrepTool: Search file contents by regex
  - LS: List directory contents
  - View: Read files
  - Edit: Modify files
  - Replace: Write files
- Conversation management with history
- Environment context collection (working directory, git status, etc.)
- Syntax highlighting for code snippets
- Support for slash commands

## Project Status

This project is in active development. We've completed the following:

1. **Project Structure:**
   - Organized the code into logical modules (app, api, config, tools, tui)
   - Created a clear separation of concerns between different components
   - Set up the project with appropriate dependencies in Cargo.toml

2. **Core Functionality:**
   - Implemented CLI argument parsing with clap
   - Added configuration management with file-based settings
   - Created conversation management for tracking message history
   - Implemented environment information collection (working directory, git status, etc.)
   - Designed a prompt system based on Claude Code's prompts

3. **API Integration:**
   - Built a Claude API client that supports both streaming and non-streaming responses
   - Implemented message formatting for Claude's API
   - Added tool definitions for Claude to use

4. **Tools Implementation:**
   - Bash command execution with security filtering
   - File search with glob patterns
   - Content search with regex
   - Directory listing
   - File reading, editing, and writing

5. **Terminal UI:**
   - Created a terminal UI using the ratatui library
   - Implemented message display with syntax highlighting
   - Added input handling for user messages
   - Designed a tool call handler for processing Claude's responses

### Current Limitations

The project has several compilation errors that need to be fixed before it can be used. Key issues include:

- Module structure issues (utils module, API submodules)
- TUI rendering issues (Frame generic parameters, regex escaping)
- API client issues (stream handling, regex usage)
- App structure issues (access to private fields)
- Command-line arg handling issues

See [TODO.md](TODO.md) for a detailed list of remaining tasks.

## Installation (Future)

### Prerequisites

- Rust and Cargo
- Claude API key

### Building from Source

```bash
# Clone the repository
git clone https://github.com/yourusername/claude-code-rs.git
cd claude-code-rs

# Build the project
cargo build --release

# The binary will be available at target/release/claude-code-rs
```

## Usage (Planned)

```bash
# Run with API key from environment variable
export CLAUDE_API_KEY=your_api_key
claude-code-rs

# Run with explicit API key
claude-code-rs --api-key your_api_key

# Run in a specific directory
claude-code-rs --directory /path/to/project

# Initialize configuration
claude-code-rs init
```

### Slash Commands

- `/help`: Show help information
- `/compact`: Compact the conversation history
- `/clear`: Clear the conversation history
- `/exit`: Exit the application

## Configuration

Configuration will be stored in `~/.config/claude-code-rs/config.json`. You will be able to set the following options:

- `api_key`: Your Claude API key
- `model`: The Claude model to use
- `history_size`: Number of conversations to keep in history

## Vision

The completed Claude Code RS will provide a powerful, terminal-based interface for pair programming with Claude. Users will be able to:

- Chat with Claude about programming tasks
- Allow Claude to view and search through their codebase
- Let Claude make edits to files directly
- Execute commands through Claude
- Have context-aware conversations about code

This tool will bring the power of Claude to the command line, making it accessible to developers who prefer terminal-based workflows.

## License

MIT

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.