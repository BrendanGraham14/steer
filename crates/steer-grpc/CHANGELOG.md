# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.12.0...steer-grpc-v0.13.0) - 2026-02-17

### Fixed

- *(api)* type stream provider errors and normalize status mapping
- *(stream)* retry transient failures and emit reset deltas

## [0.12.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.11.1...steer-grpc-v0.12.0) - 2026-02-14

### Added

- add automatic context window compaction
- *(proto,grpc,tui)* expose llm usage updates across clients

### Fixed

- *(compaction,tui)* preserve model compaction boundary while keeping history visible
- *(compaction)* persist summary boundaries across replay and session restore
- emit CompactResult on compaction failure and add auto-compaction tests

### Other

- strengthen llm usage coverage across core and grpc

## [0.11.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.10.1...steer-grpc-v0.11.0) - 2026-02-14

### Added

- preserve image attachments in queued messages and message editing
- *(steer-grpc)* wire image content through grpc conversions
- *(steer-grpc)* accept structured user content in runtime send path
- *(steer-proto)* add image content schema and send content field
- *(steer-core)* add image content variants to conversation model

### Other

- just fix
- *(steer-core)* add image api integration coverage
- *(tests)* silence tracing output in tests by default

## [0.10.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.9.0...steer-grpc-v0.10.0) - 2026-02-12

### Other

- just fix

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.8.2...steer-grpc-v0.9.0) - 2026-02-12

### Added

- *(notifications)* centralize focus-aware OSC9 notifications
- *(tui)* show agent in status bar
- *(queue)* add durable queued work and UI preview
- add create session params dto
- add policy overrides to grpc
- add server default model rpc and model resolution integration tests
- add dispatch agent approval patterns
- add primary agent mode switching
- persist session config updates
- add allow tool approval behavior
- *(core)* introduce typed system context
- *(workspace)* add git worktree orchestration
- *(tools)* add ToolSpec contract and display names
- *(tools)* align contracts and typed execution errors
- *(auth)* integrate auth plugins
- *(auth)* add grpc auth flow endpoints
- *(agents)* add dispatch session reuse
- *(workspace)* add repo tracking and repo APIs
- *(workspace)* add status commands in cli and tui
- *(agent)* support new workspaces in dispatch
- *(workspace)* add orchestration managers and lineage
- *(grpc)* add workspace/environment management RPCs
- *(auth)* codex oauth flow wiring
- move last_event_sequence to GetSession header for early subscription
- add session default model support
- redesign tool approval policy to struct-based system
- implement MCP server lifecycle effects
- split message added events by role
- *(core,proto,grpc)* add compact result event and drop model_changed
- *(steer-core)* implement slash command reducer and add compaction types
- *(steer-tui)* replace /clear with /new command for session reset
- *(streaming)* implement true SSE streaming for Anthropic provider
- *(core)* implement command handlers, MCP lifecycle, and remove legacy modules
- *(tools)* migrate all tools to static tool system with ModelCaller
- *(grpc)* add client_api module with ClientEvent and ClientCommand
- *(grpc)* integrate RuntimeAgentService into service_host and local_server
- *(core)* add SessionCatalog and SessionCreated event for session metadata

### Fixed

- make queued input editable again on cancel
- lints
- *(compaction)* persist results and simplify UI output
- resolve lints
- clean grpc error notices and reducer validation
- preserve full input schemas
- ensure event resubscribe after session switch
- roundtrip gemini thought signatures
- *(dispatch_agent)* align workspace target plumbing
- *(dispatch_agent)* reconcile formatter/executor output shape
- add resume_session to AgentClient for session resumption
- order stream deltas with events
- resolve clippy warnings
- lints, tests
- restore typed bash command flow
- stabilize deltas and drop legacy content
- *(rpc)* resolve tool approvals and simplify cancel
- *(tui,grpc)* refresh delta rendering and add compaction e2e test
- *(runtime/grpc)* align compact-result action and conversion flow

### Other

- just fix
- decouple tui from core
- add client api auth types and migrate tui
- cover planner and dispatch approvals
- just fix
- *(steer-workspace)* pass env id in list workspaces
- make ModelId a struct
- *(workspace)* remove soft-delete support
- *(workspace)* allow overriding local workspace root
- drop service host default model
- remove unused attachments field from SendMessageRequest
- rename conversation -> message_graph
- remove unused runtime paths
- update model catalog
- *(proto,grpc,core)* drop stream delta is_first and make models explicit
- *(core)* remove legacy session/event infrastructure
- *(tools)* remove LocalBackend in favor of static tool system
- *(core)* add AgentInterpreter with EventStore dependency and parent_session_id support
- *(grpc)* restrict conversions module to pub(crate)
- remove dead code and unused fields
- migrate CLI session commands to new domain types and delete legacy gRPC code
- *(core)* remove global OnceCell for tool approval channel
- *(grpc)* restrict conversion functions to pub(crate)

## [0.5.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.4.0...steer-grpc-v0.5.0) - 2025-08-19

### Added

- add support for a model display_name
- expose provider auth status via gRPC and switch TUI to remote provider registry
- *(models)* Introduce data-driven model registry
- expose ListModels and ListProviders endpoints

### Other

- merge models & providers into a single catalog file
- *(core,grpc,cli)* [**breaking**] inject ProviderRegistry and centralize AppConfig creation
- core app holds model registry, tui lists/resolves models via grpc
- generate constants for builtin models

## [0.1.17](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.1.16...steer-grpc-v0.1.17) - 2025-07-29

### Other

- *(workspace)* delete dead container code + pass working_dir as a parm

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.1.7...steer-grpc-v0.1.8) - 2025-07-24

### Added

- mcp status tracking + some tool refactoring
- *(tui)* always use detailed view of todos

### Other

- simplify tui by passing grpc client in directly
- dead code
