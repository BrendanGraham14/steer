# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.1](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.14.0...steer-tui-v0.14.1) - 2026-02-19

### Added

- *(tui)* hide read_file contents in detailed view

### Fixed

- *(core)* harden grep cancellation and clear stale pending tool state

## [0.13.1](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.12.0...steer-tui-v0.13.1) - 2026-02-17

### Fixed

- *(api)* type stream provider errors and normalize status mapping
- *(stream)* retry transient failures and emit reset deltas
- clippy

### Other

- release v0.13.0

## [0.13.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.12.0...steer-tui-v0.13.0) - 2026-02-17

### Fixed

- *(api)* type stream provider errors and normalize status mapping
- *(stream)* retry transient failures and emit reset deltas
- clippy

## [0.12.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.11.1...steer-tui-v0.12.0) - 2026-02-14

### Added

- add automatic context window compaction
- *(tui)* show remaining context in status bar
- *(proto,grpc,tui)* expose llm usage updates across clients

### Fixed

- *(compaction,tui)* preserve model compaction boundary while keeping history visible
- *(compaction)* persist summary boundaries across replay and session restore
- emit CompactResult on compaction failure and add auto-compaction tests
- clear LLM usage stats when context window changes

## [0.11.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.10.1...steer-tui-v0.11.0) - 2026-02-14

### Added

- preserve image attachments in queued messages and message editing
- *(steer-tui)* preview image attachments inline in input
- *(steer-tui)* support Ctrl+V clipboard image attachments
- *(steer-tui)* complete image paste attachment send flow
- *(steer-tui)* scaffold image paste attachment flow
- *(steer-core)* add image content variants to conversation model

### Fixed

- *(steer-tui)* skip image token labels before pushing chars to output
- *(tui)* avoid panic truncating unicode diff summary

### Other

- *(steer-core)* add image api integration coverage

## [0.10.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.9.0...steer-tui-v0.10.0) - 2026-02-12

### Other

- just fix

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.8.2...steer-tui-v0.9.0) - 2026-02-12

### Added

- *(notifications)* centralize focus-aware OSC9 notifications
- *(tui)* auto-scroll on view toggle and session restore
- rename planner agent to plan
- *(tui)* show agent in status bar
- *(tui)* add ctrl+d cancel/exit binding
- *(tui)* style input panel background
- *(queue)* add durable queued work and UI preview
- add create session params dto
- mark custom primary agents in tui
- improve scrolling
- add primary agent mode switching
- *(tui)* add edit-mode styling and cancel keybind
- *(tui)* add edit selection overlay
- tweak tui styling
- *(tools)* align contracts and typed execution errors
- *(auth)* integrate auth plugins
- *(auth)* add grpc auth flow endpoints
- *(agents)* add dispatch session reuse
- *(subagent)* support workspace target
- *(workspace)* add repo tracking and repo APIs
- *(workspace)* add status commands in cli and tui
- *(workspace)* add orchestration managers and lineage
- *(auth)* codex oauth flow wiring
- add session default model support
- implement MCP server lifecycle effects
- split message added events by role
- *(core,proto,grpc)* add compact result event and drop model_changed
- *(steer-tui)* replace /clear with /new command for session reset
- *(tools)* migrate all tools to static tool system with ModelCaller
- *(tui)* migrate TUI to use ClientEvent from client_api

### Fixed

- make queued input editable again on cancel
- lints
- *(compaction)* persist results and simplify UI output
- resolve lints
- clean grpc error notices and reducer validation
- *(tui)* align non-accent chat row indentation
- *(tui)* smooth scrolling bounds and gaps
- *(tui)* remove system/tool accent bars
- *(tui)* inset user message background
- esc to escape edit mode
- *(tui)* free ctrl+e in edit mode
- *(tui)* prioritize confirm-exit border and allow bash edits
- make it more clear that a message was edited
- ensure event resubscribe after session switch
- roundtrip gemini thought signatures
- *(dispatch_agent)* align workspace target plumbing
- *(agent)* process outputs FIFO to avoid stepper deadlock
- *(dispatch_agent)* reconcile formatter/executor output shape
- add resume_session to AgentClient for session resumption
- lints, tests
- restore typed bash command flow
- render standalone tool results
- stabilize deltas and drop legacy content
- *(tui,grpc)* refresh delta rendering and add compaction e2e test
- *(runtime/grpc)* align compact-result action and conversion flow

### Other

- just fix
- decouple tui from core
- add client api auth types and migrate tui
- use tool contracts in tui
- *(tui)* cover scroll gaps and clamping
- *(tui)* remove input panel edit selection widget
- just fix
- make ModelId a struct
- *(workspace)* allow overriding local workspace root
- drop service host default model
- update model catalog
- *(proto,grpc,core)* drop stream delta is_first and make models explicit
- *(core)* add AgentInterpreter with EventStore dependency and parent_session_id support
- migrate OneShotRunner to RuntimeService architecture

## [0.8.1](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.8.0...steer-tui-v0.8.1) - 2025-11-30

### Fixed

- stop update loop spin

### Other

- stream tui events

## [0.8.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.7.0...steer-tui-v0.8.0) - 2025-08-28

### Fixed

- clean up terminal properly in error states

### Other

- *(tui)* stabilize cache keys and reduce re-rendering in chat widgets
- message rendering benches

## [0.7.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.6.0...steer-tui-v0.7.0) - 2025-08-21

### Fixed

- remove 'U' from keybinds for update

## [0.6.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.5.0...steer-tui-v0.6.0) - 2025-08-19

### Added

- *(tui)* add GitHub update checker and status bar badge

### Other

- cleaner tool error propagation

## [0.5.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.4.0...steer-tui-v0.5.0) - 2025-08-19

### Added

- *(core,tui)* [**breaking**] improve model UX and resolution; stricter alias/display_name validation
- add support for a model display_name
- expose provider auth status via gRPC and switch TUI to remote provider registry
- *(models)* Introduce data-driven model registry
- *(core)* [**breaking**] refactor API client factory for provider-based dispatch
- make wrapped markdown list content start at the same place as text in initial line

### Other

- merge models & providers into a single catalog file
- core app holds model registry, tui lists/resolves models via grpc
- generate constants for builtin models

## [0.3.0](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.2.0...steer-tui-v0.3.0) - 2025-08-07

### Fixed

- *(tui)* move pending tool call to active

## [0.1.19](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.1.18...steer-tui-v0.1.19) - 2025-07-31

### Fixed

- respect the --model flag

## [0.1.16](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.1.15...steer-tui-v0.1.16) - 2025-07-27

### Added

- *(markdown)* treat soft breaks like hard breaks
- small cleanup for todo list formatting

### Fixed

- prevent integer overflow when scrolling with usize::MAX offset

### Other

- break input panel into separate widgets

## [0.1.15](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.1.14...steer-tui-v0.1.15) - 2025-07-25

### Other

- tweak compact description

## [0.1.9](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.1.8...steer-tui-v0.1.9) - 2025-07-24

### Added

- better diff display

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.1.7...steer-tui-v0.1.8) - 2025-07-24

### Added

- show unformatted html
- mcp status tracking + some tool refactoring
- *(tui)* unify/tidy todo formatting
- *(tui)* always use detailed view of todos

### Other

- simplify tui by passing grpc client in directly
- dead code
- a few more renames

## [0.1.7](https://github.com/BrendanGraham14/steer/compare/steer-tui-v0.1.6...steer-tui-v0.1.7) - 2025-07-23

### Other

- update Cargo.toml dependencies
