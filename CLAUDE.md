# Commands

cargo test
cargo run -p conductor-cli
cargo build


# Commits
Follow the conventional commits format.


# Conventions
- Prefer to use well-typed errors vs. using strings or stringifying them into anyhow errors. Prefer to preserve the original error rather than swallowing it and re-raising a new error type. Swallowing errors and raising new ones in their stead typically means that we'll lose information about the root cause of the error.
- Algebraic data types are useful. Use them where appropriate to write more ergonomic, well-typed programs.
- You generally should not implement the `Default` trait for structs unless explicitly instructed.
- DO NOT unwrap errors. Use the try operator `?` and propagate errors appropriately.
- When adding new packages, prefer to use `cargo add`, rather than editing Cargo.toml.
- The workspace Cargo.toml uses glob pattern `crates/*` to include all crates in the workspace.


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

