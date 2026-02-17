# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.12.0...steer-proto-v0.13.0) - 2026-02-17

### Fixed

- *(stream)* retry transient failures and emit reset deltas

## [0.12.0](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.11.1...steer-proto-v0.12.0) - 2026-02-14

### Added

- add automatic context window compaction
- *(proto,grpc,tui)* expose llm usage updates across clients

### Fixed

- *(compaction)* persist summary boundaries across replay and session restore
- emit CompactResult on compaction failure and add auto-compaction tests

## [0.11.0](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.10.1...steer-proto-v0.11.0) - 2026-02-14

### Added

- preserve image attachments in queued messages and message editing
- *(steer-proto)* add image content schema and send content field

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.8.2...steer-proto-v0.9.0) - 2026-02-12

### Added

- *(queue)* add durable queued work and UI preview
- add create session params dto
- add policy overrides to grpc
- add server default model rpc and model resolution integration tests
- add dispatch agent approval patterns
- add primary agent mode switching
- add allow tool approval behavior
- *(workspace)* add git worktree orchestration
- *(tools)* add ToolSpec contract and display names
- *(auth)* add grpc auth flow endpoints
- *(agents)* add dispatch session reuse
- *(workspace)* add repo tracking and repo APIs
- *(workspace)* add orchestration managers and lineage
- *(grpc)* add workspace/environment management RPCs
- *(workspace)* add VCS info and jj support
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

### Fixed

- make queued input editable again on cancel
- resolve lints
- *(dispatch_agent)* align workspace target plumbing
- *(dispatch_agent)* reconcile formatter/executor output shape
- restore typed bash command flow
- stabilize deltas and drop legacy content
- *(rpc)* resolve tool approvals and simplify cancel
- *(runtime/grpc)* align compact-result action and conversion flow

### Other

- remove expects/unwraps from build.rs
- clippy configs
- cover planner and dispatch approvals
- *(workspace)* move tool ops into workspace
- *(workspace)* remove soft-delete support
- prefer agents.md over claude.md
- remove unused attachments field from SendMessageRequest
- *(proto)* drop cancel operation_id field
- *(proto,grpc,core)* drop stream delta is_first and make models explicit

## [0.6.0](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.5.0...steer-proto-v0.6.0) - 2025-08-19

### Other

- cleaner tool error propagation

## [0.5.0](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.4.0...steer-proto-v0.5.0) - 2025-08-19

### Added

- expose provider auth status via gRPC and switch TUI to remote provider registry
- expose ListModels and ListProviders endpoints

### Other

- core app holds model registry, tui lists/resolves models via grpc

## [0.1.20](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.1.19...steer-proto-v0.1.20) - 2025-07-31

### Other

- vendored protoc

## [0.1.17](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.1.16...steer-proto-v0.1.17) - 2025-07-29

### Other

- *(workspace)* delete dead container code + pass working_dir as a parm

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-proto-v0.1.7...steer-proto-v0.1.8) - 2025-07-24

### Added

- mcp status tracking + some tool refactoring
- *(tui)* always use detailed view of todos
