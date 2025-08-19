# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
