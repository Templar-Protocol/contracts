#!/usr/bin/env bash
# Unit test coverage (excludes slow WASM integration tests)
#
# Usage: ./script/coverage.sh [html|lcov|text] <nextest-filter>

set -e

filter="${2:?Usage: $0 [html|lcov|text] <nextest-filter>}"
nextest_args=(--workspace --lib --ignore-default-filter -E "$filter")

echo "🔧 Installing cargo-llvm-cov if needed..."
if ! command -v cargo-llvm-cov &> /dev/null; then
    cargo install cargo-llvm-cov
fi

echo "🧪 Running unit tests with coverage..."

export SQLX_OFFLINE=true
export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Cinstrument-coverage"

# --lib: run unit tests only (excludes tests/ integration tests)
case "${1:-html}" in
    html)
        cargo llvm-cov nextest "${nextest_args[@]}" --html --open
        ;;
    lcov)
        cargo llvm-cov nextest "${nextest_args[@]}" --lcov --output-path coverage.lcov
        echo "📊 Coverage report saved to coverage.lcov"
        ;;
    text)
        cargo llvm-cov nextest "${nextest_args[@]}"
        ;;
    *)
        echo "Usage: $0 [html|lcov|text]"
        exit 1
        ;;
esac

echo "✅ Coverage complete!"
