# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0](https://github.com/BrendanGraham14/steer/compare/steer-v0.9.0...steer-v0.10.0) - 2026-02-12

### Added

- *(telemetry)* emit startup usage events

### Other

- just fix

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-v0.8.2...steer-v0.9.0) - 2026-02-12

### Added

- *(notifications)* centralize focus-aware OSC9 notifications
- add gpt-5.3-codex and opus-4.6
- add create session params dto
- reject system_prompt in session config
- add server default model rpc and model resolution integration tests
- add dispatch agent approval patterns
- tweak tui styling
- *(workspace)* add repo tracking and repo APIs
- *(workspace)* add status commands in cli and tui
- *(agent)* support new workspaces in dispatch
- *(workspace)* add orchestration managers and lineage
- *(grpc)* add workspace/environment management RPCs
- *(auth)* codex oauth flow wiring
- allow remote session create
- add session default model support
- redesign tool approval policy to struct-based system
- *(tools)* migrate all tools to static tool system with ModelCaller
- *(grpc)* integrate RuntimeAgentService into service_host and local_server

### Fixed

- lints
- resolve lints
- add resume_session to AgentClient for session resumption
- resolve session model via catalogs

### Other

- just fix
- decouple tui from core
- cover planner and dispatch approvals
- just fix
- *(workspace)* move tool ops into workspace
- make ModelId a struct
- *(workspace)* remove soft-delete support
- *(workspace)* allow overriding local workspace root
- create session via local grpc
- drop service host default model
- drop session create model flag
- remove unused runtime paths
- *(proto,grpc,core)* drop stream delta is_first and make models explicit
- *(core)* remove legacy session/event infrastructure
- *(tools)* remove LocalBackend in favor of static tool system
- *(core)* add AgentInterpreter with EventStore dependency and parent_session_id support
- migrate OneShotRunner to RuntimeService architecture
- migrate CLI session commands to new domain types and delete legacy gRPC code

## [0.8.2](https://github.com/BrendanGraham14/steer/compare/steer-v0.8.1...steer-v0.8.2) - 2025-12-02

### Fixed

- respect preferred model on startup
- handle editor args in preferences edit

## [0.8.0](https://github.com/BrendanGraham14/steer/compare/steer-v0.7.0...steer-v0.8.0) - 2025-08-28

### Fixed

- clean up terminal properly in error states

## [0.7.0](https://github.com/BrendanGraham14/steer/compare/steer-v0.6.0...steer-v0.7.0) - 2025-08-21

### Other

- just fix

## [0.5.0](https://github.com/BrendanGraham14/steer/compare/steer-v0.4.0...steer-v0.5.0) - 2025-08-19

### Added

- catalog discovery, --catalog flag, and session config auto-
- *(models)* Introduce data-driven model registry
- implement ModelRegistry with config loading and merge logic

### Other

- merge models & providers into a single catalog file
- tweak model helptext
- *(core,grpc,cli)* [**breaking**] inject ProviderRegistry and centralize AppConfig creation
- a few more hardcoded strings
- core app holds model registry, tui lists/resolves models via grpc
- generate constants for builtin models
- *(auth)* rename AuthTokens to OAuth2Token with backward compatibility

## [0.2.0](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.21...steer-v0.2.0) - 2025-08-01

### Fixed

- respect default_model preference

### Other

- support cargo binstall

## [0.1.21](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.20...steer-v0.1.21) - 2025-07-31

### Other

- update Cargo.lock dependencies

## [0.1.19](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.18...steer-v0.1.19) - 2025-07-31

### Fixed

- respect the --model flag
- display session timestamps in local timezone

### Other

- support more installers

## [0.1.17](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.16...steer-v0.1.17) - 2025-07-29

### Other

- *(workspace)* delete dead container code + pass working_dir as a parm

## [0.1.12](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.11...steer-v0.1.12) - 2025-07-25

### Other

- update Cargo.lock dependencies

## [0.1.10](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.9...steer-v0.1.10) - 2025-07-24

### Other

- update Cargo.lock dependencies

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-v0.1.7...steer-v0.1.8) - 2025-07-24

### Other

- simplify tui by passing grpc client in directly
