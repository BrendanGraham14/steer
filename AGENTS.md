# Commands

**IMPORTANT**: All build, test, and development commands MUST be run inside `nix develop` to ensure the correct toolchain and dependencies are available.

```bash
nix develop --command just check
nix develop --command just test
nix develop --command just build

# For multiple commands
nix develop --command sh -c "just check && just test"
```

This ensures the Nix environment is properly loaded without requiring an interactive shell session.

## When to use which command

- **`just check`** - Fast feedback loop: runs `cargo clippy` and `cargo fmt --check`. Use this during development.
- **`just test`** - Runs the test suite. Use after making functional changes.
- **`just build`** - Builds the project. Use when you need to verify compilation.
- **`just ci`** - Runs a full `nix flake check`, which is comprehensive but slow. **Agents should almost never need this.** It's primarily for CI pipelines and final verification before merging. Prefer `just check` and `just test` for normal development.


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

# Type-Safe Identifiers

When using primitive types (integers, UUIDs, strings) as identifiers, **always wrap them in dedicated newtype structs** to ensure type safety.

```rust
// BAD: Raw primitives are easily confused
fn get_user(user_id: i64, org_id: i64) -> User { ... }
get_user(org_id, user_id); // Compiles but wrong!

// GOOD: Newtype wrappers prevent mistakes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrgId(pub i64);

fn get_user(user_id: UserId, org_id: OrgId) -> User { ... }
get_user(org_id, user_id); // Compile error!
```

**When to use typed identifiers:**
- Database primary/foreign keys
- External API identifiers (Stripe IDs, GitHub IDs, etc.)
- Session/request/correlation IDs
- Any identifier that could be confused with another of the same primitive type

**Recommended derives:**
- `Debug`, `Clone`, `Copy` (for small types), `PartialEq`, `Eq`, `Hash`
- `serde::{Serialize, Deserialize}` with `#[serde(transparent)]` for seamless JSON
- `sqlx::Type` or equivalent for database types

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

# Tool Contract (steer-tools ↔ steer-core)

**steer-tools is the contract.** It owns tool names, display names, param/result types, and tool-specific execution errors. steer-core implements behavior without redefining the contract.

**Rules**
- Tool names and schemas live in `crates/steer-tools/src/tools/*`. steer-core must import constants/types from `steer_tools::tools::*` (no duplicate name definitions or param structs in core).
- Each tool contract defines a `ToolSpec` implementation (name, display_name, params/result/error types). steer-core static tools must reference the contract `ToolSpec` instead of redefining metadata.
- `ToolSchema` includes `display_name`; always populate it (fallback to `name` only for legacy data). Descriptions live in steer-core and may be implementation-specific.
- Tool-specific execution errors live in `crates/steer-tools/src/error.rs` as `ToolExecutionError` variants, with per-tool enums in each tool module. These are the only details allowed for execution failures.
- steer-core static tools return `ToolError::Execution(ToolExecutionError::<Tool>(...))` for execution failures, and must not invent new error kinds.
- `ToolError::InvalidParams` always includes `{ tool_name, message }`. There is no `ToolError::Io`; map raw I/O failures to `ToolExecutionError::External` (or a tool-specific `...Error::Workspace(...)` where applicable).
- Validation/approval/cancellation/timeouts are separate top-level `ToolError` variants; execution details belong under `ToolExecutionError` only.
- Workspace failures should be mapped to `WorkspaceOpError` and wrapped in the appropriate tool-specific error enum.

**Adding a new tool**
1. Define the contract in `crates/steer-tools/src/tools/<tool>.rs`: `*_TOOL_NAME`, a `ToolSpec` impl (name, display_name, params/result/error types), and a tool-specific error enum.
2. Export it from `crates/steer-tools/src/tools/mod.rs`.
3. Implement the tool in `crates/steer-core/src/tools/static_tools/<tool>.rs` using the contract types.
4. Register the tool in `crates/steer-core/src/tools/factory.rs`.
5. If it is read-only, include it in `crates/steer-core/src/tools/static_tools/mod.rs` `READ_ONLY_TOOL_NAMES`.


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

## Workspace Orchestration (Environments + Workspaces)
Steer separates execution environments (sandboxes) from VCS workspaces (jj
workspaces or git worktrees) that live inside them. An environment can host
multiple repos; each repo can have multiple jj workspaces or git worktrees,
while no-VCS repos are treated as single-workspace only.

Key points:
- Identifiers are type-safe newtypes: `EnvironmentId`, `RepoId`, `WorkspaceId`.
- `RepoRef` and `WorkspaceRef` scope repos/workspaces to an environment for
  session affinity and UI grouping.
- Workspace orchestration supports jj workspaces and git worktrees; no-VCS
  repositories remain single-workspace only.
- Managers live in `steer-workspace` (provider); client access goes through
  `steer-grpc` and `steer-remote-workspace` (no in-process shortcuts).

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
