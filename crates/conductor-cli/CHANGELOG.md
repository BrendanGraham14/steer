# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/BrendanGraham14/conductor/releases/tag/conductor-cli-v0.1.0) - 2025-07-22

### Added

- add authentication setup flow and preferences management
- add bash command approval patterns and refactor approval system
- initial support for themes + some presets
- support grok
- support importing api keys to keyring
- Introduce LlmConfigProvider and refactor config handling
- anthropic oauth support
- path as parameter to local workspace
- support more mcp backends
- default to opus
- session config toml file support
- add thread support for message editing and conversation branching
- add configurable session database path

### Fixed

- limit
- spawn notifications in a subprocess
- clippy warnings
- support latest session alias

### Other

- centralize workspace package metadata and dependencies
- message into struct with enum as data
- update reqwest
- rustls
- configure dist
- drop thread_id and rely solely on parent_message_id ancestry
- remove dead code
- debug logs for config loading
- limit sessions to 20
- clean up dead code
- Revert "chore: support tokio console subscriber"
- support tokio console subscriber
- various fixes for just ci
- remove ratatui from cli
- drop schema feature, just enable by default
- just fix
- improve error handling
- some tidyup around tokio tasks
- more strict workspace setup
- clean up
- update architecture, in_memory -> local_server
- rename crates + move them under crates/
