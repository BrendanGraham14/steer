
default:
    @just --list

fix *args:
    cargo fix --all-features --allow-staged {{args}}
    cargo clippy --fix --all-features --allow-staged {{args}} -- -D warnings
    cargo fmt

test:
    cargo test --all-features

test-package package:
    cargo test -p {{package}} --all-features

clippy:
    cargo clippy --all-features -- -D warnings

check:
    cargo check --all-features

build:
    cargo build --all-features

release:
    cargo build --release --all-features

run *args:
    cargo run --bin steer -- {{args}}

clean:
    cargo clean

fmt:
    cargo fmt --all

ci:
    nix flake check

schema-gen:
    cargo run -p steer --bin schema-generator

test-specific test_name:
    cargo test --all-features {{test_name}}

test-mcp:
    cargo test -p steer-core --lib --all-features mcp_backend

# Open the most recently modified log file in ~/.steer
log:
    #!/bin/bash
    latest_file=$(ls -t ~/.steer/*.log 2>/dev/null | grep -E '/[0-9]{8}_[0-9]{6}\.log$' | head -1)
    if [ -z "$latest_file" ]; then
        echo "No log files matching pattern YYYYMMDD_HHMMSS.log found in ~/.steer"
        exit 1
    fi
    less "$latest_file"

# Open the most recently created log file in ~/.steer
log-created:
    #!/bin/bash
    # Use GNU stat to get birth time (creation time)
    latest_file=$(find ~/.steer -name "*.log" -type f | grep -E '/[0-9]{8}_[0-9]{6}\.log$' | while read f; do stat --format="%W %n" "$f"; done | sort -n | tail -1 | cut -d' ' -f2-)
    if [ -z "$latest_file" ]; then
        echo "No log files matching pattern YYYYMMDD_HHMMSS.log found in ~/.steer"
        exit 1
    fi
    less "$latest_file"

# Configure git hooks to use the repo-local hooks in scripts/githooks
configure-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    # Ensure we run inside the repo root
    REPO_ROOT="$(git rev-parse --show-toplevel)"
    cd "$REPO_ROOT"
    HOOKS_DIR="scripts/githooks"
    echo "Configuring git hooks path to $HOOKS_DIR"
    git config core.hooksPath "$HOOKS_DIR"
    echo "Done. Pre-commit hooks will now run automatically."
