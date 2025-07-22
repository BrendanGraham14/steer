# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/BrendanGraham14/conductor/releases/tag/conductor-grpc-v0.1.0) - 2025-07-22

### Added

- implement non-destructive conversation branching and thread-aware compaction
- *(grpc)* implement streaming for large session state endpoints
- *(arch)* introduce conductor-workspace crate
- fuzzy-find commands
- Introduce unified operation tracking model
- add authentication setup flow and preferences management
- add bash command approval patterns and refactor approval system
- Introduce LlmConfigProvider and refactor config handling
- anthropic oauth support
- path as parameter to local workspace
- Refactor slash command parsing and handling
- support more mcp backends
- initial mcp support
- add fzf-like fuzzy-finder support
- initial edit support
- add thread support for message editing and conversation branching
- add configurable session database path

### Fixed

- don't use llm_format in tui

### Other

- centralize workspace package metadata and dependencies
- message into struct with enum as data
- expose ActiveMessageIdChanged event
- drop thread_id and rely solely on parent_message_id ancestry
- formatting
- remove dead code
- proto structure
- various fixes for just ci
- just fix
- improve error handling
- some tidyup around tokio tasks
- more strict workspace setup
- use well-typed outputs for tools
- clean up
- sessino manager: small cleanup
- update architecture, in_memory -> local_server
- rename crates + move them under crates/
