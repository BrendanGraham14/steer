#!/usr/bin/env bash
# configure_hooks.sh
# Sets up git to use the repo-local hooks in scripts/githooks

set -euo pipefail

# Ensure we run inside the repo root
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

HOOKS_DIR="scripts/githooks"

echo "Configuring git hooks path to $HOOKS_DIR"

git config core.hooksPath "$HOOKS_DIR"

echo "Done. Pre-commit hooks will now run automatically."
