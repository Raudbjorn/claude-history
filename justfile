# Rust project development commands

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# List available commands
default:
    @just --list

# Verify the system Python embedding dependency
python-deps:
    @python -c 'import fastembed; print(f"python-fastembed {fastembed.__version__}")'

# Build the debug binary
build:
    cargo build --locked

# Type-check every target
check:
    cargo check --locked --all-targets

# Run all tests with an isolated home for application caches
test:
    test_home="$(mktemp -d)"; trap 'rm -rf "$test_home"' EXIT; CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" HOME="$test_home" cargo test --locked

# Format Rust sources
fmt:
    cargo fmt

# Check Rust source formatting
fmt-check:
    cargo fmt --check

# Run Clippy
lint:
    cargo clippy --locked --all-targets

# Run the complete local verification suite
verify: python-deps fmt-check lint test

# Install the release binary globally
install: python-deps
    cargo install --offline --path . --locked

# Install the debug binary globally via symlink
install-dev: python-deps
    cargo build --locked
    mkdir -p ~/.cargo/bin
    ln -sf "$(pwd)/target/debug/claude-history" ~/.cargo/bin/claude-history

# Run the application
run *ARGS: python-deps
    cargo run --locked -- "$@"

# Remove Cargo build artifacts
clean:
    cargo clean

# Verify, bump, tag, and publish a release
release bump="patch": verify
    cargo release {{bump}} --execute
