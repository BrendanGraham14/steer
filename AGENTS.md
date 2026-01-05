# Commands

just check        # Quick compilation check without building
just test         # Run all tests with all features
just run          # Run steer CLI (can add args like: just run --help)
just build        # Build the project with all features
just ci           # Run all checks (fmt, clippy, test)
just fix          # Auto-fix issues and format code

# Other useful commands:
# just              # Show all available commands
# just release      # Build release version
# just test-package steer-core  # Test specific package
# just test-specific test_name      # Run specific test
# just fmt          # Format code
# just clippy       # Run clippy linting
# just clean        # Clean build artifacts

# Nix commands:
# nix develop       # Enter development shell
# nix build         # Build the project with Nix (uses crane for better caching)
# nix flake check   # Run all checks
# nix run           # Run steer directly
# nix build .#steer                  # Build just the CLI
# nix build .#steer-remote-workspace # Build just the remote workspace
# nix build .#steer-workspace        # Build all crates at once


# Version Control
This project uses `jj` (Jujutsu) for version control.

Create new commits frequently as checkpoints during development. Small, incremental commits make it easier to track progress, revert problematic changes, and understand the evolution of the codebase. Don't wait until a feature is complete—commit working states along the way.

# Commits
Follow the conventional commits format.


# Conventions
- NEVER use anyhow errors. Always use well-typed errors with thiserror.
- If you need to convert errors to a fuzzy representation for user-facing messages, use eyre, NEVER anyhow.
- Prefer to preserve the original error rather than swallowing it and re-raising a new error type. Swallowing errors and raising new ones in their stead typically means that we'll lose information about the root cause of the error.
- Algebraic data types are useful. Use them where appropriate to write more ergonomic, well-typed programs.
- You generally should not implement the `Default` trait for structs unless explicitly instructed.
- In production code, DO NOT unwrap errors. Use the try operator `?` and propagate errors appropriately. In tests, `.unwrap()` and `panic!` are allowed for brevity, but prefer `assert!` or `expect()` with descriptive messages where possible.
- NEVER use panic! in production code. Handle errors properly instead of panicking.
- When adding new packages, prefer to use `cargo add`, rather than editing Cargo.toml.
- The workspace Cargo.toml uses glob pattern `crates/*` to include all crates in the workspace.

# UI Theming
- **NEVER hardcode colors** in TUI code. Always use the theme system to fetch colors.
- Use `theme.component_style(Component::...)` to get the appropriate style for UI components.
- This ensures consistent theming and allows users to customize their color schemes.


# Error Handling

**IMPORTANT**: Never use `anyhow` for error handling in this codebase. All errors must be well-typed using `thiserror`.

The steer-core crate uses typed errors with `thiserror`. The main error type is `crate::error::Error` which contains variants for all core error types:
- `Api(ApiError)` - For API-related errors
- `Session(SessionError)` - For session management errors  
- `Tool(ToolError)` - For tool execution errors
- `Validation(ValidationError)` - For input validation errors
- `Io(std::io::Error)` - For I/O operations
- And other domain-specific variants

Each module typically defines its own error type (e.g., `ApiError`, `SessionError`) that gets included in the main `Error` enum. This approach provides better error context and type safety compared to string-based errors.

**Note**: If you need to present errors to users in a more readable format, use `eyre` for error reporting at the UI boundary, but NEVER use `anyhow`. The core logic should always use typed errors.


# Crate Architecture (Polylith-style)

All crates are organized under the `crates/` directory following a polylith-style architecture:
- `crates/steer-proto/` - Protocol buffer definitions and generated code
- `crates/steer-core/` - Core domain logic (LLM APIs, session management, tool execution backends)
- `crates/steer-grpc/` - gRPC server/client implementation
- `crates/steer/` - Command-line interface and main binary
- `crates/steer-tui/` - Terminal UI library (ratatui-based)
- `crates/steer-tools/` - Tool trait definitions and implementations
- `crates/steer-macros/` - Procedural macros for tool definitions
- `crates/steer-workspace/` - Workspace trait and local workspace implementation (provider)
- `crates/steer-workspace-client/` - Remote workspace client implementation (consumer)
- `crates/steer-remote-workspace/` - gRPC service for remote workspace operations

## Nix Development Environment

The project includes a Nix flake for reproducible development environments. To preserve shell aliases and configurations:

1. **Using nix-direnv** (recommended): Install with `nix profile install nixpkgs#nix-direnv`, then `direnv allow`. This preserves your parent shell environment.

2. **Custom shell config**: Copy `.steer-shell.nix.example` to `.steer-shell.nix` and customize it with your preferred aliases and configurations.

3. **Direct sourcing**: The flake's shellHook automatically sources your `.zshrc`/`.bashrc` to preserve aliases.

## Dependency Graph
The crates must maintain a strict acyclic dependency graph:
```
steer-tools → steer-workspace → steer-core → steer-grpc → clients (tui, cli, etc.)
         ↓                                      ↑
steer-macros                   steer-workspace-client
```

Key principles:
- Provider/consumer separation: workspace trait (provider) is separate from remote client (consumer)
- No circular dependencies: each crate has clear, one-way dependencies
- Tool execution (backends) is separate from workspace operations

## Crate Responsibilities

### steer-proto
- ONLY contains .proto files and tonic-generated code
- Defines the stable wire protocol
- No business logic whatsoever

### steer-workspace
- Defines the Workspace trait for environment and filesystem operations
- Provides LocalWorkspace implementation
- NO tool execution logic - purely environment/filesystem focused
- Exports EnvironmentInfo, WorkspaceConfig types

### steer-workspace-client
- RemoteWorkspace client implementation
- Depends on steer-workspace for trait definitions
- Uses gRPC to communicate with steer-remote-workspace service

### steer-core
- Pure domain logic (LLM APIs, session management, validation)
- Tool execution backends (LocalBackend, McpBackend)
- MUST NOT:
  - Import steer-grpc
  - Return or accept proto types in public APIs
  - Know about networking, UI, or transport concerns
- CAN import steer-proto for basic type definitions only
- Imports steer-workspace for workspace operations
- Exports domain types (Message, SessionConfig) and traits (AppCommandSink, AppEventSource)

### steer-grpc
- Network transport layer only
- The ONLY crate that knows about both core types and proto types
- Contains ALL conversions between core domain types ↔ proto messages
- Conversion functions must be pub(crate), never public
- Implements gRPC server/client that wrap core functionality

### steer-remote-workspace
- gRPC service implementing remote workspace operations
- Wraps a LocalWorkspace to expose it over the network
- Used by RemoteWorkspace clients

### Client crates (steer-tui, etc.)
- MUST go through steer-grpc, never directly access steer-core
- Even "local" mode should use an in-memory gRPC channel
- No special treatment - all clients are equal

## Important Rules

1. **No proto types in core**: If you see `steer_proto::` in steer-core outside of basic imports, it's a violation
2. **All conversions in grpc**: Any function converting between core and proto types belongs in steer-grpc
3. **No in-process shortcuts**: Clients must always use the gRPC interface, even for local operations
4. **Clean boundaries**: Each crate has a single, clear responsibility
5. **Path references**: When referencing files outside crates (e.g., `include_str!`), remember to account for the extra directory level: `../../` becomes `../../../` or `../../../../`
6. **Workspace vs Tools**: Workspace handles environment/filesystem, backends handle tool execution - never mix these concerns
7. **Provider/Consumer split**: Workspace trait provider (steer-workspace) is separate from remote consumer (steer-workspace-client)

