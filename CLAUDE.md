# Commands

just check        # Quick compilation check without building
just test         # Run all tests with all features
just run          # Run conductor CLI (can add args like: just run --help)
just build        # Build the project with all features
just ci           # Run all checks (fmt, clippy, test)
just fix          # Auto-fix issues and format code

# Other useful commands:
# just              # Show all available commands
# just release      # Build release version
# just test-package conductor-core  # Test specific package
# just test-specific test_name      # Run specific test
# just fmt          # Format code
# just clippy       # Run clippy linting
# just clean        # Clean build artifacts

# Nix commands:
# nix develop       # Enter development shell
# nix build         # Build the project with Nix (uses crane for better caching)
# nix flake check   # Run all checks
# nix run           # Run conductor directly
# nix build .#conductor-cli              # Build just the CLI
# nix build .#conductor-remote-workspace # Build just the remote workspace
# nix build .#conductor-workspace        # Build all crates at once


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


# Error Handling

**IMPORTANT**: Never use `anyhow` for error handling in this codebase. All errors must be well-typed using `thiserror`.

The conductor-core crate uses typed errors with `thiserror`. The main error type is `crate::error::Error` which contains variants for all core error types:
- `Api(ApiError)` - For API-related errors
- `Session(SessionError)` - For session management errors  
- `Tool(ToolError)` - For tool execution errors
- `Validation(ValidationError)` - For input validation errors
- `Io(std::io::Error)` - For I/O operations
- And other domain-specific variants

Each module typically defines its own error type (e.g., `ApiError`, `SessionError`) that gets included in the main `Error` enum. This approach provides better error context and type safety compared to string-based errors.

**Note**: If you need to present errors to users in a more readable format, use `eyre` for error reporting at the UI boundary, but NEVER use `anyhow`. The core logic should always use typed errors.


# Crate Architecture

All crates are organized under the `crates/` directory:
- `crates/conductor-proto/` - Protocol buffer definitions and generated code
- `crates/conductor-core/` - Core domain logic (LLM APIs, session management, tool execution)
- `crates/conductor-grpc/` - gRPC server/client implementation
- `crates/conductor-cli/` - Command-line interface and main binary
- `crates/conductor-tui/` - Terminal UI library (ratatui-based)
- `crates/conductor-tools/` - Tool trait definitions and implementations
- `crates/conductor-macros/` - Procedural macros for tool definitions
- `crates/conductor-remote-workspace/` - gRPC service for remote tool execution

## Nix Development Environment

The project includes a Nix flake for reproducible development environments. To preserve shell aliases and configurations:

1. **Using nix-direnv** (recommended): Install with `nix profile install nixpkgs#nix-direnv`, then `direnv allow`. This preserves your parent shell environment.

2. **Custom shell config**: Copy `.conductor-shell.nix.example` to `.conductor-shell.nix` and customize it with your preferred aliases and configurations.

3. **Direct sourcing**: The flake's shellHook automatically sources your `.zshrc`/`.bashrc` to preserve aliases.

## Dependency Graph
The crates must maintain a strict acyclic dependency graph:
```
conductor-proto → conductor-core → conductor-grpc → clients (tui, cli, etc.)
```

## Crate Responsibilities

### conductor-proto
- ONLY contains .proto files and tonic-generated code
- Defines the stable wire protocol
- No business logic whatsoever

### conductor-core
- Pure domain logic (LLM APIs, session management, validation, tool execution)
- MUST NOT:
  - Import conductor-grpc
  - Return or accept proto types in public APIs
  - Know about networking, UI, or transport concerns
- CAN import conductor-proto for basic type definitions only
- Exports domain types (Message, SessionConfig) and traits (AppCommandSink, AppEventSource)

### conductor-grpc
- Network transport layer only
- The ONLY crate that knows about both core types and proto types
- Contains ALL conversions between core domain types ↔ proto messages
- Conversion functions must be pub(crate), never public
- Implements gRPC server/client that wrap core functionality

### Client crates (conductor-tui, etc.)
- MUST go through conductor-grpc, never directly access conductor-core
- Even "local" mode should use an in-memory gRPC channel
- No special treatment - all clients are equal

## Important Rules

1. **No proto types in core**: If you see `conductor_proto::` in conductor-core outside of basic imports, it's a violation
2. **All conversions in grpc**: Any function converting between core and proto types belongs in conductor-grpc
3. **No in-process shortcuts**: Clients must always use the gRPC interface, even for local operations
4. **Clean boundaries**: Each crate has a single, clear responsibility
5. **Path references**: When referencing files outside crates (e.g., `include_str!`), remember to account for the extra directory level: `../../` becomes `../../../` or `../../../../`

