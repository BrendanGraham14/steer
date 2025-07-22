# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/BrendanGraham14/conductor/releases/tag/conductor-remote-workspace-v0.1.0) - 2025-07-22

### Added

- *(arch)* introduce conductor-workspace crate
- initial mcp support
- add fzf-like fuzzy-finder support

### Fixed

- don't omit dotfiles by default
- replace blocking std::process::Command with git2 crate
- don't use llm_format in tui

### Other

- centralize workspace package metadata and dependencies
- swap out git2 for gix
- remove default git2 deps to remove dependency on openssl
- configure dist
- proto structure
- various fixes for just ci
- remove anyhow from remote-workspace
- just fix
- some tidyup around tokio tasks
- more strict workspace setup
- use well-typed outputs for tools
- clean up
- rename crates + move them under crates/
