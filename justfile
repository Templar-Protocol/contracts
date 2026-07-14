# Templar contract-mvp — project tasks. Run `just` to list recipes.

# Show available recipes.
default:
    @just --list

# Format SQL (standalone files + inline in Rust).
sql-fmt:
    sleek $(find . -name '*.sql' -not -path './target/*')
    ./script/sql-fmt-inline.pl $(find . -name '*.rs' -not -path './target/*')

# Format Rust + SQL.
fmt: sql-fmt
    cargo fmt

# Run the full test suite (prebuilds wasms, starts postgres); args pass to nextest.
test *args:
    ./script/test.sh {{args}}

# Run the node-backed tests against a pool of out-of-band neard nodes. The
# `sandbox` profile selects them (see .config/nextest.toml); args pass to nextest.
test-sandbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    trap './script/sandbox-down.sh || true' EXIT
    cargo nextest run --profile sandbox {{args}}

# Start the out-of-band sandbox neard (prints its RPC url).
sandbox-up:
    ./script/sandbox-up.sh

# Stop the out-of-band sandbox neard.
sandbox-down:
    ./script/sandbox-down.sh

# Generate HTML coverage.
coverage:
    ./script/coverage.sh html

# Build the docs.
docs:
    ./script/build-docs.sh
