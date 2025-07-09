
default:
    @just --list

fix:
    cargo fix --all-features --allow-staged
    cargo clippy --fix --all-features --allow-staged -- -D warnings
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
    cargo run --bin conductor -- {{args}}

clean:
    cargo clean

fmt:
    cargo fmt --all

ci:
    nix flake check

schema-gen:
    cargo run -p conductor-cli --bin schema-generator

test-specific test_name:
    cargo test --all-features {{test_name}}

test-mcp:
    cargo test -p conductor-core --lib --all-features mcp_backend
