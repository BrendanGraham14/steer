# Claude Code RS - Architecture

This document outlines the architecture of the Claude Code RS application, both in its current state and the planned improvements.

## Overview

Claude Code RS is a terminal-based application for pair programming with Claude. It allows users to interact with Claude in a command-line interface, with support for various tools that Claude can use to help with coding tasks.

## Application Layers

The application is structured into the following layers:

### 1. API Layer (`src/api/`)
- Responsible for communication with the Claude API
- Handles authentication, request formatting, and response parsing
- Manages streaming responses from Claude
- Formats tool calls and tool results according to Claude API requirements

### 2. App Layer (`src/app/`)
- Core application logic
- Manages application state
- Coordinates between the API, tools, and UI
- Handles message processing and tool execution via ToolExecutor

### 3. Tools Layer (`src/tools/`)
- Implementations of various tools (bash, glob, grep, ls, view, edit, replace)
- Each tool encapsulates its specific functionality
- Tools communicate with the App layer, not directly with UI

### 4. UI Layer (`src/tui/`)
- Terminal user interface using ratatui
- Handles user input and display
- Renders messages and handles formatting

### 5. Configuration (`src/config/`)
- Manages application configuration
- Handles loading and saving settings

### 6. Utilities (`src/utils/`)
- Common utility functions used across the application
- Provides logging and error handling facilities

## Current Architecture

The application follows a layered architecture with clear separation of concerns:

```
                    ┌──────────────┐
                    │              │
                    │    Tools     │
                    │              │
                    └──────────────┘
                          ▲
                          │
┌──────────────┐     ┌──────────────┐
│              │     │              │
│     API      │◀───▶│     App      │
│              │     │              │
└──────────────┘     └──────────────┘
                          ▲
                          │
                          ▼
                    ┌──────────────┐
                    │     TUI      │
                    │              │
                    └──────────────┘
```

### Message Flow

The current message flow is:

1. User enters a message in the TUI
2. TUI sends the message to the App layer
3. App layer adds the message to the conversation
4. App layer sends the conversation to the API layer
5. API layer sends the messages to Claude and receives a response
6. API layer returns the response to the App layer
7. App layer processes the response:
   - Adds the assistant's message to the conversation
   - If tool calls are present, processes them via ToolExecutor
   - Formats tool results as special messages in the conversation
   - If necessary, continues the conversation with tool results
8. App layer sends events to the TUI for display

## Message Tracking and Recent Improvements

Recent improvements focused on fixing tool response handling with the Claude API:

1. **Proper Tool Result Formatting**:
   - Tool results are now properly sent to Claude as user messages with a specialized JSON structure
   - Empty tool results are handled gracefully and don't cause API errors

2. **Message Content Validation**:
   - Added checks to prevent empty messages from being sent to the API
   - Added safeguards to ensure tool results are never empty or malformed

## Message Handling Protocol

The system uses a standardized message handling protocol with robust ID tracking to maintain consistent message order and display.

### 1. Message ID Structure

Each message has a unique ID with the following structure:
- **Message Type Prefix**: "user_", "assistant_", "tool_", "system_"
- **Timestamp**: Unix timestamp in seconds
- **Optional Counter**: A random or sequential component to avoid collisions
- **Format**: `{prefix}{timestamp}_{counter}`

Example: `assistant_1709557832_a43f` for an assistant message

### 2. Event Types

The system uses specific event types to distinguish between message creation and updates:

```rust
pub enum AppEvent {
    // Create a new message
    MessageAdded {
        role: Role,
        content: String,
        id: String,
    },
    
    // Update an existing message (for streaming or modifications)
    MessageUpdated {
        id: String,
        content: String,
    },
    
    // Tool events
    ToolCallStarted { name: String, id: Option<String> },
    ToolCallCompleted { name: String, result: String, id: Option<String> },
    ToolCallFailed { name: String, error: String, id: Option<String> },
    
    // Processing indicators
    ThinkingStarted,
    ThinkingCompleted,
    
    // Command and error handling
    CommandResponse { content: String, id: Option<String> },
    Error { message: String },
}
```

### 3. Message Lifecycle Protocol

The protocol for message handling follows these rules:

1. **Message Creation**: 
   - Each new message emits exactly one `MessageAdded` event
   - Message IDs are unique and follow the required format
   - System messages are created but filtered from UI display

2. **Streaming Updates**:
   - Initial empty assistant message is created with `MessageAdded`
   - All subsequent content chunks use `MessageUpdated` with the same ID
   - The protocol prevents duplicate `MessageAdded` events for the same message

3. **Tool Execution**:
   - Tool calls follow the sequence: `ToolCallStarted` → `ToolCallCompleted` or `ToolCallFailed`
   - Tool results appear as separate messages with their own IDs
   - Tool messages include references to their parent message ID for tracking

4. **UI Event Processing**:
   - UI processes events by ID, not by role or order received
   - For `MessageAdded`: Creates a new message in the display
   - For `MessageUpdated`: Finds the existing message by ID and updates content
   - UI maintains a lookup table of message IDs for efficient updates

### 4. Implementation Details

The message handling implementation includes:

1. **App Layer**:
   - Generates unique, collision-resistant message IDs
   - Emits the correct event type (Added vs Updated) based on message lifecycle
   - During streaming, updates the same message rather than creating duplicates
   - Tracks relationships between messages, tools, and their results

2. **UI Layer**:
   - Processes events in the order received
   - Maintains a fast lookup system for message IDs
   - Applies updates to the correct message based on ID matching
   - Uses ID-based lookups instead of role-based heuristics
   - Filters system messages from display

3. **Synchronization Process**:
   - Uses robust synchronization mechanisms for shared state
   - Implements ordering guarantees for message updates
   - Logs detailed events for debugging message flow

### 5. Message Data Model

The message data model supports this protocol:

```rust
pub struct Message {
    pub id: String,          // Unique identifier
    pub role: Role,          // User, Assistant, Tool, System
    pub content: String,     // Message content
    pub timestamp: u64,      // Creation timestamp
    pub parent_id: Option<String>, // Optional reference to parent message
}

pub struct FormattedMessage {
    pub id: String,          // Same ID as the original message
    pub role: Role,          // Same role as the original message
    pub content: Vec<Line>,  // Formatted content for display
}
```

This protocol ensures consistent message handling across the application.