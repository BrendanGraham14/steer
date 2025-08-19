# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
