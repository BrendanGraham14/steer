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
- **Current issue**: Tool call parsing needs to be improved for streaming responses

### 2. App Layer (`src/app/`)
- Core application logic
- Manages application state
- Coordinates between the API, tools, and UI
- **Planned improvement**: Should be responsible for tool execution, not the UI

### 3. Tools Layer (`src/tools/`)
- Implementations of various tools (bash, glob, grep, ls, view, edit, replace)
- Each tool encapsulates its specific functionality
- **Current state**: Tools are executed directly from the UI layer

### 4. UI Layer (`src/tui/`)
- Terminal user interface using ratatui
- Handles user input and display
- **Current issue**: Currently has too much responsibility, including executing tools

### 5. Configuration (`src/config/`)
- Manages application configuration
- Handles loading and saving settings

### 6. Utilities (`src/utils/`)
- Common utility functions used across the application

## Current Architecture Issues

The primary architectural issue is that the UI layer (TUI) is currently responsible for executing tools, which violates the separation of concerns principle:

1. The TUI module directly calls tool execution functions
2. The TUI parses and processes tool calls from Claude's responses
3. The TUI manages the flow of tool execution results back to Claude

This creates several problems:
- Makes the UI code complex and difficult to test
- Tightly couples the UI to the business logic
- Makes it harder to switch to a different UI in the future
- Complicates error handling

## Planned Architecture Improvements

### 1. Tool Execution Refactoring

Move tool execution responsibility from the UI layer to the App layer:

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

1. Create a `ToolExecutor` in the App layer that will:
   - Receive tool calls from the API client
   - Execute the appropriate tools
   - Manage the results
   - Handle retries and errors

2. Update the API client to properly parse tool calls and pass them to the App layer

3. Simplify the TUI to focus only on:
   - Displaying messages and results
   - Collecting user input
   - Rendering the interface

### 2. Message Flow

The revised message flow will be:

1. User enters a message in the TUI
2. TUI sends the message to the App layer
3. App layer sends the message to the API layer
4. API layer sends the message to Claude and receives a response
5. API layer returns the response to the App layer
6. If the response contains tool calls:
   - App layer identifies and processes the tool calls
   - App layer executes the tools using the ToolExecutor
   - App layer sends the results back to the API layer to continue the conversation
7. App layer passes the final response to the TUI for display

This ensures that:
- TUI only communicates with the App layer, never directly with the API
- App layer manages all the business logic and state
- Tool execution is handled in the appropriate layer
- Each layer has clear, single responsibilities

### 3. Error Handling

Improve error handling throughout the application:

1. API layer: Handle API errors and connection issues
2. App layer: Handle tool execution errors and state management issues
3. TUI layer: Handle display and user input errors

## Implementation Plan

1. ✅ Create a ToolExecutor in the App layer
2. ✅ Update the API client to parse tool calls correctly
3. ✅ Refactor the TUI to remove tool execution logic
4. ✅ Update message passing between layers
5. ✅ Implement proper error handling

## Implemented Changes (March 2025)

1. **ToolExecutor**:
   - Created a dedicated ToolExecutor class in the App layer
   - Implemented single and parallel tool execution methods
   - Added tool call ID support for better tracking

2. **API Client Improvements**:
   - Added methods to extract tool calls from Claude responses
   - Implemented text extraction from mixed-content responses
   - Added helper methods to detect and process tool calls

3. **App Layer Enhancements**:
   - Added methods for executing tools through the ToolExecutor
   - Added methods for sending messages and handling tools
   - Implemented command handling in the App layer
   - Added conversation management methods

4. **TUI Simplification**:
   - Removed direct API client dependency
   - Removed direct tool execution code
   - Made TUI delegate all business logic to App layer
   - Simplified message flow through App layer

5. **Proper Messaging Flow**:
   - TUI now only handles display and user input
   - App layer coordinates all conversation flow
   - App layer manages tool execution and results
   - API client handles Claude communication details

This refactoring has created a cleaner architecture with proper separation of concerns, making the codebase more maintainable, testable, and extensible.