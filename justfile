# Conductor project justfile

# Default recipe - show available recipes
default:
    @just --list

fix:
    cargo fix --all-features --allow-staged
    cargo clippy --all-features --allow-staged
    cargo fmt --allow-staged

# Run all tests with all features
test:
    cargo test --all-features

# Run tests for a specific package
test-package package:
    cargo test -p {{package}} --all-features

# Run clippy with all features
clippy:
    cargo clippy --all-features -- -D warnings

# Check the project with all features
check:
    cargo check --all-features

# Build the project with all features
build:
    cargo build --all-features

# Build release version
release:
    cargo build --release --all-features

# Run conductor CLI
run *args:
    cargo run -p conductor-cli -- {{args}}

# Clean build artifacts
clean:
    cargo clean

# Format code
fmt:
    cargo fmt --all

# Run all checks (fmt, clippy, test)
ci: fmt clippy test

# Generate schema files
schema:
    cargo run -p conductor-cli --bin schema_generator --features schema

# Run specific test with all features
test-specific test_name:
    cargo test --all-features {{test_name}}

# Run MCP backend tests specifically
test-mcp:
    cargo test -p conductor-core --lib --all-features mcp_backend
