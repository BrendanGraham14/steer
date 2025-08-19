# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.4.0...steer-grpc-v0.5.0) - 2025-08-19

### Added

- add support for a model display_name
- expose provider auth status via gRPC and switch TUI to remote provider registry
- *(models)* Introduce data-driven model registry
- expose ListModels and ListProviders endpoints

### Other

- merge models & providers into a single catalog file
- *(core,grpc,cli)* [**breaking**] inject ProviderRegistry and centralize AppConfig creation
- core app holds model registry, tui lists/resolves models via grpc
- generate constants for builtin models

## [0.1.17](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.1.16...steer-grpc-v0.1.17) - 2025-07-29

### Other

- *(workspace)* delete dead container code + pass working_dir as a parm

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-grpc-v0.1.7...steer-grpc-v0.1.8) - 2025-07-24

### Added

- mcp status tracking + some tool refactoring
- *(tui)* always use detailed view of todos

### Other

- simplify tui by passing grpc client in directly
- dead code
