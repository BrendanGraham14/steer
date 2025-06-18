# Coder - Architecture

This document outlines the architecture of Coder, an AI-powered CLI assistant for software engineering tasks.

## Overview

Coder can operate in three modes:

1. **Local Interactive**: Terminal UI runs directly with a local App instance
2. **Headless**: One-shot execution that outputs JSON (for scripting/automation)
3. **Client/Server**: Terminal UI connects to a gRPC server that hosts the App instances

## High-Level Architecture

```
┌─────────────────┐       ┌─────────────────-─┐       ┌─────────────────┐
│   Terminal UI   │       │  gRPC Client      │       │ Headless Runner │
│    (ratatui)    │       │   (--remote)      │       │  (JSON output)  │
└────────┬────────┘       └────────┬──────────┘       └────────┬────────┘
         │                         │                           │
         │ AppCommand              │ gRPC/Protobuf             │
         │ AppEvent                │                           │
         ▼                         ▼                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                           Session Manager                           │
│  ┌─────────────┐   ┌─────────────┐   ┌─────────────┐                │
│  │   Session   │   │   Session   │   │   Session   │    SQLite      │
│  │  (App + ID) │   │  (App + ID) │   │  (App + ID) │    Storage     │
│  └──────┬──────┘   └──────┬──────┘   └──────┬──────┘                │
└─────────┼─────────────────┼─────────────────┼───────────────────────┘
          │                 │                 │
          ▼                 ▼                 ▼
┌────────────────────────────────────────────────────────────────────────┐
│                          App Actor Loop                                │
│   - Message processing    - Tool execution    - Event emission         │
│   - LLM orchestration    - Approval handling  - State management       │
└────────────────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. Terminal UI (`coder/src/tui`)
- **ratatui-based** terminal interface
- Sends `AppCommand` messages to control the App
- Receives `AppEvent` messages for display updates
- Handles user input, text editing, and tool approval prompts
- Can connect to local App or remote gRPC server

### 2. App Actor Loop (`coder/src/app`)
The heart of Coder - an event-driven actor that:
- Manages conversation state
- Coordinates with LLM providers (Anthropic, OpenAI, Gemini)
- Executes tools through `ToolExecutor`
- Handles tool approval flow
- Emits events for UI updates

Key structures:
- `App`: Main actor struct with conversation state, tool executor, API client
- `OpContext`: Tracks active operations and enables cancellation
- `AgentExecutor`: Manages LLM API calls and streaming responses

### 3. Session Manager (`coder/src/session`)
- Multiplexes multiple App instances (sessions)
- Persists conversations to SQLite (`~/.coder/coder.sqlite`)
- Handles session lifecycle (create, resume, delete)
- Manages tool approval policies per session

### 4. Tool System (`coder/src/tools`, `tools/`)
Tools are implemented as async functions that can:
- Read/write files (`view`, `edit`, `replace`)
- Search code (`grep`, `glob`)
- Execute commands (`bash`)
- Manage tasks (`todo`)

Tool execution flow:
1. LLM requests tool via `ToolCall`
2. `ToolExecutor` checks if approval needed
3. If needed, UI prompts user for approval
4. Tool runs on selected backend (local, remote, container)
5. Result returned to LLM as `Message::Tool`

### 5. Workspace Abstraction (`coder/src/workspace`)
- **Local**: Tools execute in current directory
- **Remote**: Tools execute on remote machine via gRPC
- Provides environment context for system prompt

## gRPC Protocol

The gRPC protocol enables client/server separation using bidirectional streaming:

### Server Side (`coder serve`)
```
┌─────────────────────────┐
│    gRPC Server          │
│  ┌──────────────────┐   │
│  │ AgentServiceImpl │   │
│  └────────┬─────────┘   │
│           │             │
│  ┌────────▼─────────┐   │
│  │ SessionManager   │   │
│  └──────────────────┘   │
└─────────────────────────┘
```

The server:
- Listens on a port (default 50051)
- Hosts SessionManager with multiple App instances
- Streams events to connected clients
- Handles session persistence

### Client Side (`coder --remote`)
```
┌─────────────────────────┐
│   Terminal UI           │
│  ┌──────────────────┐   │
│  │ GrpcClientAdapter│   │
│  └────────┬─────────┘   │
│           │             │
│      gRPC │ Stream      │
│           ▼             │
└─────────────────────────┘
```

The client:
- Connects to server address
- Translates AppCommands → gRPC ClientMessages
- Translates gRPC ServerEvents → AppEvents
- Maintains bidirectional stream for real-time updates

### Protocol Flow

1. **Session Creation**:
   ```
   Client                          Server
     |----CreateSessionRequest----->|
     |<-----SessionInfo-------------|
   ```

2. **Bidirectional Streaming**:
   ```
   Client                          Server
     |----StreamSession(stream)---->|
     |----Subscribe---------------->|
     |<----MessageAddedEvent--------|
     |----SendMessage-------------->|
     |<----ThinkingStartedEvent-----|
     |<----MessagePartEvent---------|
     |<----RequestToolApproval------|
     |----ToolApprovalResponse----->|
     |<----ToolCallStartedEvent-----|
     |<----ToolCallCompletedEvent---|
   ```

3. **Message Types**:
   - **ClientMessage**: Commands from UI (SendMessage, ToolApproval, Cancel)
   - **ServerEvent**: Updates to UI (MessageAdded, ToolCallStarted, Error)

### Remote Workspace Protocol

For remote tool execution, a separate gRPC service handles workspace operations:

```proto
service RemoteWorkspaceService {
  rpc GetEnvironment(...) returns (EnvironmentInfo);
  rpc ExecuteTool(...) returns (ToolResult);
  rpc GetAvailableTools(...) returns (ToolList);
}
```

## Event Flow

### Local Mode
```
User Input → TUI → AppCommand → App Actor → LLM API
                                    ↓
Display ← AppEvent ← Tool Result ← Tool Execution
```

### Remote Mode
```
User Input → TUI → AppCommand → gRPC Client → Network → gRPC Server
                                                            ↓
                                                        App Actor
                                                            ↓
Display ← AppEvent ← gRPC Events ← Network ← Server Events
```

## Key Design Decisions

1. **Actor Model**: The App runs as an async actor with message passing, avoiding shared mutable state
2. **Event Sourcing**: All UI updates driven by events, making the system debuggable and testable
3. **Tool Abstraction**: Tools are provider-agnostic and can run on different backends
4. **Session Persistence**: SQLite provides reliable storage without external dependencies
5. **Streaming gRPC**: Enables real-time updates and interactive tool approval flows

## Data Flow Examples

### Sending a Message
1. User types message in TUI
2. TUI sends `AppCommand::ProcessUserInput`
3. App adds `Message::User` to conversation
4. App calls LLM API with conversation history
5. LLM response streams back as `MessagePart` events
6. TUI renders updates in real-time

### Tool Execution with Approval
1. LLM returns `ToolCall` in response
2. App checks if tool needs approval
3. If yes, emits `RequestToolApproval` event
4. TUI shows approval prompt
5. User approves/denies
6. App executes tool (if approved)
7. Tool result added to conversation
8. LLM continues with tool result

## File Structure

```
coder/
├── src/
│   ├── api/          # LLM provider clients
│   ├── app/          # Core App actor and logic
│   ├── cli/          # Command-line argument parsing
│   ├── commands/     # CLI subcommand implementations
│   ├── grpc/         # gRPC server and client
│   ├── session/      # Session management
│   ├── tools/        # Tool execution infrastructure
│   ├── tui/          # Terminal UI
│   └── workspace/    # Workspace abstractions
├── tools/            # Tool implementations
└── proto/           # Protocol buffer definitions
```
