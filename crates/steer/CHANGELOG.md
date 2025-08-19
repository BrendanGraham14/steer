# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
