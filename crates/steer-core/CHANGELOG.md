# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.8.2...steer-core-v0.9.0) - 2026-02-12

### Added

- *(notifications)* centralize focus-aware OSC9 notifications
- add gpt-5.3-codex and opus-4.6
- rename planner agent to plan
- *(api)* retry streams on transport errors
- *(api)* preserve typed sse parse errors
- *(queue)* add durable queued work and UI preview
- *(openai)* handle responses stream deltas
- wire primary agent policy in runtime
- mark custom primary agents in tui
- add server default model rpc and model resolution integration tests
- persist session todos in database
- add dispatch agent approval patterns
- add primary agent mode switching
- persist session config updates
- *(core)* add primary agent switching core
- *(core)* add primary agent presets
- add allow tool approval behavior
- *(core)* introduce typed system context
- *(workspace)* add git worktree orchestration
- *(tools)* enforce tool error mapping
- *(tools)* add ToolSpec contract and display names
- *(core)* include env context in main agent prompt
- *(runners)* gate tool approvals in one-shot runner
- *(tools)* align contracts and typed execution errors
- *(agents)* allow per-spec model override
- *(auth)* integrate auth plugins
- *(auth)* add grpc auth flow endpoints
- *(auth)* add plugin registry scaffold
- *(auth)* add auth plugin primitives crate
- *(agents)* add dispatch session reuse
- *(subagent)* support workspace target
- *(workspace)* add repo tracking and repo APIs
- *(workspace)* add status commands in cli and tui
- *(agent)* support new workspaces in dispatch
- *(workspace)* add orchestration managers and lineage
- *(grpc)* add workspace/environment management RPCs
- *(workspace)* add workspace manager abstractions
- *(api)* add debug logging for OpenAI responses request payloads
- *(auth)* codex oauth flow wiring
- add session default model support
- redesign tool approval policy to struct-based system
- implement MCP server lifecycle effects
- split message added events by role
- *(core,proto,grpc)* add compact result event and drop model_changed
- *(steer-core)* implement slash command reducer and add compaction types
- *(steer-tui)* replace /clear with /new command for session reset
- *(streaming)* add SSE streaming support for OpenAI, xAI, and Gemini providers
- *(catalog)* gemini-3
- *(streaming)* implement true SSE streaming for Anthropic provider
- *(core)* implement command handlers, MCP lifecycle, and remove legacy modules
- *(tools)* migrate all tools to static tool system with ModelCaller
- *(tools)* add capability-based static tool system with DI
- *(tui)* migrate TUI to use ClientEvent from client_api
- *(core)* add SessionCatalog and SessionCreated event for session metadata
- *(core)* add RuntimeService with supervisor/actor architecture
- *(core)* add RuntimeManagedSession wrapper for AppRuntime
- *(core)* complete agent loop in reducer - extract tool calls and continue after results
- *(core)* add SQLite EventStore for domain event sourcing
- *(core)* add AgentInterpreter for stepper execution
- *(core)* add pure AgentStepper state machine
- *(core)* add dual-channel dispatcher with delta coalescing
- *(core)* add AgentExecutor adapter for reducer integration

### Fixed

- make queued input editable again on cancel
- lints
- *(steer-core)* parse OpenAI responses errors with typed models
- *(steer-core)* preapprove explore dispatch in normal mode
- *(compaction)* persist results and simplify UI output
- resolve lints
- clean grpc error notices and reducer validation
- *(serialization)* avoid tool error tag collisions
- *(runtime)* handle cancellation output draining
- wire tool schema reload source
- improve agent policy resolution
- sanitize anthropic tool schemas
- sanitize gemini tool schemas
- internally tag dispatch agent workspace target
- align tool schemas with OpenAI format
- preserve full input schemas
- auto-deny malformed tool calls
- append operating mode to override prompts
- make the default tool approval policy allow all read-only tools
- *(core)* avoid async recursion in tool reload
- *(core)* avoid clearing newer operation state
- codex instructions
- roundtrip gemini thought signatures
- *(claude)* omit display_name from tool payload
- *(core)* return typed results for static tools
- *(session)* serialize tool visibility lists
- *(dispatch_agent)* align workspace target plumbing
- *(agent)* process outputs FIFO to avoid stepper deadlock
- *(dispatch_agent)* reconcile formatter/executor output shape
- *(dispatch_agent)* thread workspace through core workspace trait
- test
- add env provider abstraction
- tool call approvals
- order stream deltas with events
- resolve clippy warnings
- lints, tests
- preserve stream content order
- restore typed bash command flow
- render direct bash output as user message
- tool calling + streaming
- stabilize deltas and drop legacy content
- stop model call after direct bash
- *(runtime)* preserve pending approval model state
- *(rpc)* resolve tool approvals and simplify cancel
- *(tui,grpc)* refresh delta rendering and add compaction e2e test
- *(runtime/grpc)* align compact-result action and conversion flow

### Other

- remove expects/unwraps from build.rs
- just fix
- align sub-agent policy expectations
- pull agent names into constants
- expand claude schema sanitizer coverage
- enforce structured dispatch_agent args
- coerce dispatch_agent params
- add dispatch_agent tool call integration test
- parametrize api tests with rstest
- rename dispatch agent tags
- cover dispatch_agent schema
- default to codex instead of opus
- cover planner and dispatch approvals
- *(core)* repro missing model for operation
- just fix
- *(tools)* assert sub-agent runtime persistence
- *(runners)* cover approval auto-deny flow
- *(tools)* run sub-agents via runtime
- clean up dead workspace arg to tool executor
- steer-tools contains tool contract, core contains impl
- *(workspace)* move tool ops into workspace
- make ModelId a struct
- *(workspace)* remove soft-delete support
- prefer agents.md over claude.md
- *(openai)* merge CodexClient into OpenAIClient
- *(auth)* extract chatgpt account ID from id_token instead of access_token
- remove unused attachments field from SendMessageRequest
- rename conversation -> message_graph
- remove unused runtime paths
- update model catalog
- *(proto,grpc,core)* drop stream delta is_first and make models explicit
- *(steer-core)* split conversation.rs into submodules and rename Conversation to MessageGraph
- *(core)* remove legacy session/event infrastructure
- *(tools)* remove LocalBackend in favor of static tool system
- *(core)* remove deprecated AgentExecutor and related types
- *(core)* add domain tests and deprecate legacy AgentExecutor
- *(core)* add AgentInterpreter with EventStore dependency and parent_session_id support
- remove dead code and unused fields
- migrate OneShotRunner to RuntimeService architecture
- *(core)* remove global OnceCell for tool approval channel

## [0.8.2](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.8.1...steer-core-v0.8.2) - 2025-12-02

### Fixed

- refresh provider cache on auth failure

## [0.8.1](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.8.0...steer-core-v0.8.1) - 2025-11-30

### Other

- remove bash command filtering
- increase openai request timeout from 5 -> 30 mins

## [0.7.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.6.0...steer-core-v0.7.0) - 2025-08-21

### Fixed

- *(core)* respect thinking_config across providers

## [0.6.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.5.0...steer-core-v0.6.0) - 2025-08-19

### Other

- cleaner tool error propagation

## [0.5.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.4.0...steer-core-v0.5.0) - 2025-08-19

### Added

- *(core,tui)* [**breaking**] improve model UX and resolution; stricter alias/display_name validation
- add support for a model display_name
- expose provider auth status via gRPC and switch TUI to remote provider registry
- catalog discovery, --catalog flag, and session config auto-
- *(models)* Introduce data-driven model registry
- implement ModelRegistry with config loading and merge logic
- add modelconfig & modelparameters
- *(core)* [**breaking**] refactor API client factory for provider-based dispatch
- *(core)* implement ProviderRegistry for runtime provider loading
- *(core)* introduce provider types and compile-time defaults for auth refactor
- gpt-5 -specific prompt
- support openai responses api + codex-mini

### Fixed

- *(catalog)* some configs
- don't reference function_calls for non-claude models

### Other

- merge models & providers into a single catalog file
- *(core,grpc,cli)* [**breaking**] inject ProviderRegistry and centralize AppConfig creation
- a few more hardcoded strings
- core app holds model registry, tui lists/resolves models via grpc
- generate constants for builtin models
- support bridging between legacy Model enum and new model registry
- *(auth)* rename AuthTokens to OAuth2Token with backward compatibility
- upgrade rmcp

## [0.4.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.3.0...steer-core-v0.4.0) - 2025-08-07

### Added

- gpt-5

## [0.3.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.2.0...steer-core-v0.3.0) - 2025-08-07

### Added

- opus-4.1

## [0.2.0](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.1.21...steer-core-v0.2.0) - 2025-08-01

### Fixed

- respect default_model preference

## [0.1.19](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.1.18...steer-core-v0.1.19) - 2025-07-31

### Fixed

- respect the --model flag

## [0.1.17](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.1.16...steer-core-v0.1.17) - 2025-07-29

### Other

- *(workspace)* delete dead container code + pass working_dir as a parm

## [0.1.16](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.1.15...steer-core-v0.1.16) - 2025-07-27

### Fixed

- don't continue the conversation after compacting

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-core-v0.1.7...steer-core-v0.1.8) - 2025-07-24

### Added

- mcp status tracking + some tool refactoring

### Other

- dead code
