# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.1](https://github.com/BrendanGraham14/steer/compare/steer-tools-v0.14.0...steer-tools-v0.14.1) - 2026-02-19

### Added

- enhance subagent prompting to encourage passing more detailed context

## [0.14.0](https://github.com/BrendanGraham14/steer/compare/steer-tools-v0.13.1...steer-tools-v0.14.0) - 2026-02-18

### Added

- *(fetch)* relax URL policy and harden fetch summarization

## [0.11.0](https://github.com/BrendanGraham14/steer/compare/steer-tools-v0.10.1...steer-tools-v0.11.0) - 2026-02-14

### Other

- just fix

## [0.10.1](https://github.com/BrendanGraham14/steer/compare/steer-tools-v0.10.0...steer-tools-v0.10.1) - 2026-02-12

### Fixed

- *(dispatch-agent)* clarify sub-agent workspace context in prompts

## [0.9.0](https://github.com/BrendanGraham14/steer/compare/steer-tools-v0.8.2...steer-tools-v0.9.0) - 2026-02-12

### Added

- persist session todos in database
- *(tools)* enforce tool error mapping
- *(tools)* add ToolSpec contract and display names
- *(tools)* align contracts and typed execution errors
- *(agents)* add dispatch session reuse
- redesign tool approval policy to struct-based system
- *(tools)* migrate all tools to static tool system with ModelCaller

### Fixed

- lints
- resolve lints
- *(serialization)* avoid tool error tag collisions
- internally tag dispatch agent workspace target
- align tool schemas with OpenAI format
- preserve full input schemas
- *(tools)* restore ToolResult conversions
- *(dispatch_agent)* align workspace target plumbing
- *(dispatch_agent)* reconcile formatter/executor output shape

### Other

- just fix
- make dispatch_agent params partialeq
- rename dispatch agent tags
- cover dispatch_agent schema
- rename todo read / write tools to snake_case
- just fix
- *(workspace)* move tool ops into workspace

## [0.1.8](https://github.com/BrendanGraham14/steer/compare/steer-tools-v0.1.7...steer-tools-v0.1.8) - 2025-07-24

### Added

- mcp status tracking + some tool refactoring
- *(tui)* unify/tidy todo formatting
- *(tui)* always use detailed view of todos

### Fixed

- *(bash tool)* limit {stdout|stderr} {chars|lines}

### Other

- a few more renames
