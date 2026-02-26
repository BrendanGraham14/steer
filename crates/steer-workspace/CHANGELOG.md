# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.17.0](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.16.1...steer-workspace-v0.17.0) - 2026-02-26

### Added

- *(tools)* add raw reads and typed edit failures

## [0.16.1](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.16.0...steer-workspace-v0.16.1) - 2026-02-23

### Added

- allow switching agents while op in progress

### Fixed

- *(workspace)* base new jj workspaces on parent change

## [0.15.0](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.14.2...steer-workspace-v0.15.0) - 2026-02-20

### Other

- *(deps)* remove unused dependencies flagged by machete

## [0.14.1](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.14.0...steer-workspace-v0.14.1) - 2026-02-19

### Fixed

- *(core)* harden grep cancellation and clear stale pending tool state

### Other

- *(workspace)* streamline grep match aggregation
- *(workspace)* add grep include and cancellation regressions
- *(workspace)* optimize grep match path and mtime handling
- *(workspace)* grep perf

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.8.2...steer-workspace-v0.9.0) - 2026-02-12

### Added

- *(workspace)* add git worktree orchestration
- *(subagent)* support workspace target
- *(workspace)* add repo tracking and repo APIs
- *(grpc)* add workspace/environment management RPCs
- *(workspace)* add workspace registry
- *(workspace)* add workspace manager abstractions
- *(workspace)* add VCS info and jj support

### Fixed

- lints
- resolve lints
- preserve workspace snapshot cleanup
- mimic jj snapshot options
- *(dispatch_agent)* align workspace target plumbing
- *(agent)* process outputs FIFO to avoid stepper deadlock

### Other

- just fix
- just fix
- *(steer-workspace)* split utils modules
- *(steer-workspace)* pass env id in list workspaces
- *(steer-workspace)* extract workspace layout
- *(steer-workspace)* split manager jj helpers
- *(steer-workspace)* split local module
- *(workspace)* move tool ops into workspace
- add jj snapshot option coverage
- *(workspace)* remove soft-delete support
- *(workspace)* cover registry and manager
- prefer agents.md over claude.md

## [0.6.0](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.5.0...steer-workspace-v0.6.0) - 2025-08-19

### Other

- cleaner tool error propagation

## [0.1.18](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.1.17...steer-workspace-v0.1.18) - 2025-07-30

### Fixed

- *(workspace)* skip files/directories if we don't have access to them

## [0.1.17](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.1.16...steer-workspace-v0.1.17) - 2025-07-29

### Other

- *(workspace)* delete dead container code + pass working_dir as a parm

## [0.1.11](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.1.10...steer-workspace-v0.1.11) - 2025-07-25

### Added

- filter out .git from workspace file listing

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-workspace-v0.1.7...steer-workspace-v0.1.8) - 2025-07-24

### Added

- mcp status tracking + some tool refactoring
