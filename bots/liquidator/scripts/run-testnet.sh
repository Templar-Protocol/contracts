#!/bin/bash
# SPDX-License-Identifier: MIT
#
# Run Templar liquidator on testnet.
# Default settings run in observation mode (MIN_PROFIT_BPS=10000).
#
# USAGE:
#   cp .env.example .env
#   # Edit .env: set LIQUIDATOR_ACCOUNT and LIQUIDATOR_PRIVATE_KEY
#   ./scripts/run-testnet.sh
#
# CONFIGURATION:
#   All settings loaded from .env file. Required variables:
#   - LIQUIDATOR_ACCOUNT: Your NEAR account (e.g., liquidator.testnet)
#   - LIQUIDATOR_PRIVATE_KEY: Account private key (ed25519:...)
#
#   Testnet defaults:
#   - Registry: templar-registry.testnet
#   - Asset: nep141:usdc.testnet
#   - Swap: rhea-swap (dclv2.ref-dev.testnet)

set -e

# Load .env file
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/../.env"
if [ -f "$ENV_FILE" ]; then
    # shellcheck disable=SC1090
    source "$ENV_FILE"
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Validate required environment variables
if [ -z "$LIQUIDATOR_ACCOUNT" ]; then
    error "LIQUIDATOR_ACCOUNT not set"
    echo "  Set in .env or: export LIQUIDATOR_ACCOUNT=\"your-account.testnet\""
    exit 1
fi

if [ -z "$LIQUIDATOR_PRIVATE_KEY" ]; then
    error "LIQUIDATOR_PRIVATE_KEY not set"
    echo "  Set in .env or: export LIQUIDATOR_PRIVATE_KEY=\"ed25519:...\""
    exit 1
fi

# Configuration with testnet defaults
NETWORK="testnet"
REGISTRIES="${TESTNET_REGISTRIES:-templar-registry.testnet}"
LIQUIDATION_ASSET="${LIQUIDATION_ASSET:-nep141:usdc.testnet}"
SWAP_PROVIDER="${SWAP_PROVIDER:-rhea-swap}"
INTERVAL="${INTERVAL:-600}"
REGISTRY_REFRESH_INTERVAL="${REGISTRY_REFRESH_INTERVAL:-3600}"
CONCURRENCY="${CONCURRENCY:-10}"
PARTIAL_PERCENTAGE="${PARTIAL_PERCENTAGE:-50}"
TIMEOUT="${TIMEOUT:-60}"
MAX_GAS_PERCENTAGE="${MAX_GAS_PERCENTAGE:-10}"
MIN_PROFIT_BPS="${MIN_PROFIT_BPS:-10000}"
LOG_JSON="${LOG_JSON:-false}"

# Build binary if needed
PROJECT_ROOT="$SCRIPT_DIR/../../.."
BINARY_PATH="$PROJECT_ROOT/target/debug/liquidator"

if [ ! -f "$BINARY_PATH" ]; then
    warn "Building liquidator binary..."
    cd "$PROJECT_ROOT"
    cargo build -p templar-liquidator --bin liquidator
    if [ ! -f "$BINARY_PATH" ]; then
        error "Build failed"
        exit 1
    fi
fi

# Print configuration
echo ""
info "Templar Liquidator - Testnet"
echo ""
echo "  Network:          $NETWORK"
echo "  Account:          $LIQUIDATOR_ACCOUNT"
echo "  Registries:       $REGISTRIES"
echo "  Asset:            $LIQUIDATION_ASSET"
echo "  Swap:             $SWAP_PROVIDER"
echo "  Min Profit:       ${MIN_PROFIT_BPS} bps"
echo ""

if [ "$MIN_PROFIT_BPS" -ge 5000 ]; then
    info "✓ OBSERVATION MODE (min profit >= 50%)"
else
    warn "WARNING: Min profit is ${MIN_PROFIT_BPS} bps"
    read -p "Continue? (yes/no) " -n 3 -r
    echo
    if [[ ! $REPLY =~ ^yes$ ]]; then
        exit 0
    fi
fi

# Set log level
export RUST_LOG="${RUST_LOG:-info,templar_liquidator=debug}"

# Build command arguments
CMD_ARGS=(
    "--network" "$NETWORK"
    "--signer-account" "$LIQUIDATOR_ACCOUNT"
    "--signer-key" "$LIQUIDATOR_PRIVATE_KEY"
    "--asset" "$LIQUIDATION_ASSET"
    "--swap" "$SWAP_PROVIDER"
    "--interval" "$INTERVAL"
    "--registry-refresh-interval" "$REGISTRY_REFRESH_INTERVAL"
    "--concurrency" "$CONCURRENCY"
    "--partial-percentage" "$PARTIAL_PERCENTAGE"
    "--min-profit-bps" "$MIN_PROFIT_BPS"
    "--timeout" "$TIMEOUT"
    "--max-gas-percentage" "$MAX_GAS_PERCENTAGE"
)

for registry in $REGISTRIES; do
    CMD_ARGS+=("--registries" "$registry")
done

[ "$LOG_JSON" = "true" ] && CMD_ARGS+=("--log-json")

# Add RPC_URL if set
[ -n "$RPC_URL" ] && CMD_ARGS+=("--rpc-url" "$RPC_URL")

# Export ONECLICK_API_TOKEN if set (used by one-click-api provider)
if [ -n "$ONECLICK_API_TOKEN" ]; then
    export ONECLICK_API_TOKEN
    info "Using 1-Click API token (fee reduced to 0%)"
fi

info "Starting liquidator..."
echo ""
exec "$BINARY_PATH" "${CMD_ARGS[@]}"
