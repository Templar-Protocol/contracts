#!/usr/bin/env bash
# Simple coverage script
set -e

echo "🔧 Installing cargo-llvm-cov if needed..."
if ! command -v cargo-llvm-cov &> /dev/null; then
    cargo install cargo-llvm-cov
fi

echo "🏗️ Preparing test environment..."
./script/prebuild-test-contracts.sh

# Start database for tests
docker compose --file service/relayer/compose.dev.yaml up postgres --detach

echo "🧪 Running tests with coverage..."
export SQLX_OFFLINE=true
export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Cinstrument-coverage"

# Run with desired output format
case "${1:-html}" in
    html)
        cargo llvm-cov nextest --html --open
        ;;
    lcov)
        cargo llvm-cov nextest --lcov --output-path coverage.lcov
        echo "Coverage report saved to coverage.lcov"
        ;;
    text)
        cargo llvm-cov nextest
        ;;
    *)
        echo "Usage: $0 [html|lcov|text]"
        exit 1
        ;;
esac

echo "🧹 Cleaning up..."
docker compose --file service/relayer/compose.dev.yaml down postgres

echo "✅ Coverage complete!"
