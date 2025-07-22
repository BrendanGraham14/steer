# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/BrendanGraham14/conductor/releases/tag/conductor-core-v0.1.0) - 2025-07-22

### Added

- tweak o3 prompts, output tokens, enable gpt-4.1
- implement non-destructive conversation branching and thread-aware compaction
- *(tui)* add vim and simple editing modes
- *(arch)* introduce conductor-workspace crate
- *(model fzf)* filter weak models, boost aliases
- fuzzy-find commands
- Introduce unified operation tracking model
- compact uses current model instead of hardcoded claude-3.7-sonnet
- support grok-4
- add authentication setup flow and preferences management
- add bash command approval patterns and refactor approval system
- support grok
- cache system prompt
- *(environment)* include some ignored paths (but no children), add a max_files and max_depth limit, breadth-first
- support importing api keys to keyring
- Introduce LlmConfigProvider and refactor config handling
- anthropic oauth support
- custom gemini prompt based on gemini-cli
- *(nix)* add Nix development environment
- path as parameter to local workspace
- Refactor slash command parsing and handling
- support more mcp backends
- default to opus
- initial mcp support
- session config toml file support
- add fzf-like fuzzy-finder support
- add notification support to TUI
- initial edit support
- add thread support for message editing and conversation branching
- add configurable session database path

### Fixed

- *(gemini+mcp)* filter out fields which aren't supported by gemini api (but are tolerated by other apis)
- filter models for picker everywhere
- don't omit dotfiles by default
- some clean up on Processing handling to reduce unnecessary notifs
- serialize creds to a single keyring, delete encrypted file storage
- bug where tool message ids were getting regenerated incorrectly
- gracefully skip directories if conductor does not have permission to access them
- workspaceconfig::local
- bug where app is re-created on resume
- o4-mini alias
- edit bugs
- compilation errors after pulling
- *(gemini)* handle malformed function call finish reason
- pin rmcp
- schema feature
- replace blocking std::process::Command with git2 crate
- don't use llm_format in tui
- *(openai)* don't pass in thought content to request
- don't pass thoughts in to gemini, it mimics the [thought] format if we do

### Other

- centralize workspace package metadata and dependencies
- a bunch of tidyup around message lineage handling
- message into struct with enum as data
- small tidyup
- break things into widgets
- update reqwest
- dead test
- swap out git2 for gix
- rustls
- remove default git2 deps to remove dependency on openssl
- drop thread_id and rely solely on parent_message_id ancestry
- debug log for no content blocks error
- centralize memory file name and add tool name constants
- more verbose debug log
- remove dead code
- proto structure
- clean up dead code
- log events which fail to get broadcasted
- grok -> xai
- remove LlmConfig caching for immediate auth updates
- decouple tool approval & execution callbacks
- propagate / surface errors more clearly
- default to warn
- pull prompts into source code
- various fixes for just ci
- support debug logging messages
- remove anyhow from tui
- drop schema feature, just enable by default
- just fix
- improve error handling
- some tidyup around tokio tasks
- more strict workspace setup
- log gemini request if 400/404
- use well-typed outputs for tools
- clean up
- sessino manager: small cleanup
- rename crates + move them under crates/
