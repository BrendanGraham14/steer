# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/BrendanGraham14/conductor/releases/tag/conductor-tui-v0.1.0) - 2025-07-22

### Added

- *(tui)* conversation branch filtering
- add approval() view for tool calls, override it for edit
- *(tui)* add custom slash command support and migrate to directories crate
- *(tui)* add vim and simple editing modes
- *(arch)* introduce conductor-workspace crate
- *(model fzf)* filter weak models, boost aliases
- fuzzy-finder for model picker & theme picker
- fuzzy-find commands
- richer nav / delete / etc keybinds
- clarify approval copy
- Introduce unified operation tracking model
- wrap system notices
- markdown code syntax highlighting
- default to catppuccin-mocha
- add authentication setup flow and preferences management
- add bash command approval patterns and refactor approval system
- *(markdown)* task lists
- fork/vendor tui-markdown, support theming
- initial support for themes + some presets
- support alt+left/right
- tweak input panel style
- input panel styling improvements
- Introduce LlmConfigProvider and refactor config handling
- tool styling tweaks
- path as parameter to local workspace
- Refactor slash command parsing and handling
- support more mcp backends
- *(tui)* add ExternalFormatter for MCP tools and pretty JSON output
- *(fzf)* add a space after selected path
- styling
- support tab to accept fzf suggestion
- add fzf-like fuzzy-finder support
- markdown support + render caching
- dynamically sized input / tool approval area
- add notification support to TUI
- nicer message selection
- initial edit support
- add thread support for message editing and conversation branching

### Fixed

- padding for assistant & tool msgs
- render bullet points onseparate lines in markdown lists
- show tool name
- *(vim mode / fzf)* make / in normal mode pop open fzf
- fuzzy-finder
- *(vim mode)* state transitions to setup
- *(vim mode)* esc esc to clear & edit
- filter models for picker everywhere
- some clean up on Processing handling to reduce unnecessary notifs
- markdown lists with formatted text
- centralize auth controller creation in setup handler
- clear auth controller after entering creds
- markdown codeblock handling
- clear in_list_item_start state for empty list items
- markdown overflow
- rebase error
- straggling task
- spawn notifications in a subprocess
- clippy warnings
- edit bugs
- *(tui)* wrap desktop notifications in spawn_blocking to avoid blocking async runtime
- *(fzf)* don't override textarea scrolling
- fzf passing up/down through to textarea when unhandled
- fzf scroll direction after inverting
- tui cleanup in tests
- a few fzf bugs
- make edit selection a stateful widget
- trim command
- don't truncate bash command
- don't use llm_format in tui
- render full command output
- casing
- display tool status, tidy up formatting
- scroll by 1
- tool approval action key styling
- overflow
- support paste in bash mode

### Other

- centralize workspace package metadata and dependencies
- a bunch of tidyup around message lineage handling
- message into struct with enum as data
- widgets now only expose lines(), chat viewport renders
- small tidyup
- borrow chatitems
- break things into widgets
- drop thread_id and rely solely on parent_message_id ancestry
- replace vec-based ChatStore with IndexMap for stable key-based access
- proto structure
- don't log info about cancellation
- grok -> xai
- propagate / surface errors more clearly
- better keybindings
- more cleanup
- *(tui)* use tokio::time::interval for spinner animation
- various fixes for just ci
- remove anyhow from tui
- remove direct crossterm dep
- just fix
- *(tui)* replace actually_beep with notify-rust's native sound support
- improve error handling
- more strict workspace setup
- fzf helpers
- pull input panel and status bar into widgets, pull tui keystroke handlers into separate files
- pull out input_panel rendering function
- dead code
- unify caching for chat items
- use well-typed outputs for tools
- clean up
- add edit to copy
- add hover indicator for message to edit
- rename crates + move them under crates/
