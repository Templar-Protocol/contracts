#!/bin/bash
# SPDX-License-Identifier: MIT
#
# Run Templar liquidator on mainnet.
# Default settings run in DRY RUN mode (DRY_RUN=true).
#
# USAGE:
#   cp .env.example .env
#   # Edit .env: set LIQUIDATOR_ACCOUNT and LIQUIDATOR_PRIVATE_KEY
#   ./scripts/run-mainnet.sh
#
# CONFIGURATION:
#   All settings loaded from .env file. Required variables:
#   - LIQUIDATOR_ACCOUNT: Your NEAR account (e.g., liquidator.near)
#   - LIQUIDATOR_PRIVATE_KEY: Account private key (ed25519:...)
#
#   Optional overrides available - see .env.example for full list.
#
# SAFETY:
#   Default DRY_RUN=true prevents any liquidations (scan and log only).
#   For production: Set DRY_RUN=false and MIN_PROFIT_BPS=50-200 (0.5-2%)
#
# CONTRACT ADDRESSES:
#   See: ../../docs/src/deployments.md
#   - Registry: v1.tmplr.near
#   - Oracle: pyth-oracle.near
#   - USDC: nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1

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
    echo "  Set in .env or: export LIQUIDATOR_ACCOUNT=\"your-account.near\""
    exit 1
fi

if [ -z "$LIQUIDATOR_PRIVATE_KEY" ]; then
    error "LIQUIDATOR_PRIVATE_KEY not set"
    echo "  Set in .env or: export LIQUIDATOR_PRIVATE_KEY=\"ed25519:...\""
    exit 1
fi

# Configuration with defaults (see .env.example for all options)
NETWORK="${NETWORK:-mainnet}"
REGISTRIES="${MAINNET_REGISTRIES:-v1.tmplr.near}"
LIQUIDATION_ASSET="${LIQUIDATION_ASSET:-nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1}"
SWAP_PROVIDER="${SWAP_PROVIDER:-near-intents}"
INTERVAL="${INTERVAL:-600}"
REGISTRY_REFRESH_INTERVAL="${REGISTRY_REFRESH_INTERVAL:-3600}"
CONCURRENCY="${CONCURRENCY:-10}"
PARTIAL_PERCENTAGE="${PARTIAL_PERCENTAGE:-50}"
TIMEOUT="${TIMEOUT:-60}"
MAX_GAS_PERCENTAGE="${MAX_GAS_PERCENTAGE:-10}"
MIN_PROFIT_BPS="${MIN_PROFIT_BPS:-10000}"
LOG_JSON="${LOG_JSON:-false}"
DRY_RUN="${DRY_RUN:-true}"

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
info "Templar Liquidator - Mainnet"
echo ""
echo "  Network:          $NETWORK"
echo "  Account:          $LIQUIDATOR_ACCOUNT"
echo "  Registries:       $REGISTRIES"
echo "  Asset:            ${LIQUIDATION_ASSET:0:20}..."
echo "  Swap:             $SWAP_PROVIDER"
echo "  Min Profit:       ${MIN_PROFIT_BPS} bps"
echo "  Dry Run:          $DRY_RUN"
echo ""

if [ "$DRY_RUN" = "true" ]; then
    info "✓ DRY RUN MODE (scan and log only, no liquidations)"
else
    warn "WARNING: DRY_RUN=false - This WILL execute liquidations!"
    warn "Min profit threshold: ${MIN_PROFIT_BPS} bps (${MIN_PROFIT_BPS}% = $((MIN_PROFIT_BPS/100))% profit)"
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
[ "$DRY_RUN" = "true" ] && CMD_ARGS+=("--dry-run")

# Add RPC_URL if set
[ -n "$RPC_URL" ] && CMD_ARGS+=("--rpc-url" "$RPC_URL")

info "Starting liquidator..."
echo ""
exec "$BINARY_PATH" "${CMD_ARGS[@]}"
