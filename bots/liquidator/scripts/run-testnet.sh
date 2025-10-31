#!/bin/bash
# SPDX-License-Identifier: MIT
#
# Run Templar liquidator on testnet.
# Default settings run in observation mode (MIN_PROFIT_BPS=10000).
#
# USAGE:
#   cp .env.example .env
#   # Edit .env: set SIGNER_ACCOUNT_ID and SIGNER_KEY
#   ./scripts/run-testnet.sh
#
# CONFIGURATION:
#   All settings loaded from .env file. Required variables:
#   - SIGNER_ACCOUNT_ID: Your NEAR account (e.g., liquidator.testnet)
#   - SIGNER_KEY: Account private key (ed25519:...)
#
#   Testnet defaults:
#   - Registry: templar-registry.testnet
#   - Liquidation Strategy: partial (50%)

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
if [ -z "$SIGNER_ACCOUNT_ID" ]; then
    error "SIGNER_ACCOUNT_ID not set"
    echo "  Set in .env or: export SIGNER_ACCOUNT_ID=\"your-account.testnet\""
    exit 1
fi

if [ -z "$SIGNER_KEY" ]; then
    error "SIGNER_KEY not set"
    echo "  Set in .env or: export SIGNER_KEY=\"ed25519:...\""
    exit 1
fi

# Configuration with testnet defaults
NETWORK="testnet"
REGISTRIES="${REGISTRY_ACCOUNT_IDS:-templar-registry.testnet}"
LIQUIDATION_STRATEGY="${LIQUIDATION_STRATEGY:-partial}"
LIQUIDATION_SCAN_INTERVAL="${LIQUIDATION_SCAN_INTERVAL:-600}"
REGISTRY_REFRESH_INTERVAL="${REGISTRY_REFRESH_INTERVAL:-3600}"
INVENTORY_REFRESH_INTERVAL="${INVENTORY_REFRESH_INTERVAL:-300}"
CONCURRENCY="${CONCURRENCY:-10}"
PARTIAL_PERCENTAGE="${PARTIAL_PERCENTAGE:-50}"
TRANSACTION_TIMEOUT="${TRANSACTION_TIMEOUT:-60}"
MAX_GAS_PERCENTAGE="${MAX_GAS_PERCENTAGE:-10}"
MIN_PROFIT_BPS="${MIN_PROFIT_BPS:-10000}"
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
info "Templar Liquidator - Testnet (Inventory-Based)"
echo ""
echo "  Network:              $NETWORK"
echo "  Account:              $SIGNER_ACCOUNT_ID"
echo "  Registries:           $REGISTRIES"
echo "  Liquidation Strategy: $LIQUIDATION_STRATEGY"
echo "  Min Profit:           ${MIN_PROFIT_BPS} bps"
echo "  Dry Run:              $DRY_RUN"
echo ""

if [ "$DRY_RUN" = "true" ]; then
    info "✓ DRY RUN MODE (scan and log only, no liquidations)"
elif [ "$MIN_PROFIT_BPS" -ge 5000 ]; then
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
    "--signer-account" "$SIGNER_ACCOUNT_ID"
    "--signer-key" "$SIGNER_KEY"
    "--liquidation-strategy" "$LIQUIDATION_STRATEGY"
    "--liquidation-scan-interval" "$LIQUIDATION_SCAN_INTERVAL"
    "--registry-refresh-interval" "$REGISTRY_REFRESH_INTERVAL"
    "--inventory-refresh-interval" "$INVENTORY_REFRESH_INTERVAL"
    "--concurrency" "$CONCURRENCY"
    "--partial-percentage" "$PARTIAL_PERCENTAGE"
    "--min-profit-bps" "$MIN_PROFIT_BPS"
    "--transaction-timeout" "$TRANSACTION_TIMEOUT"
    "--max-gas-percentage" "$MAX_GAS_PERCENTAGE"
)

for registry in $REGISTRIES; do
    CMD_ARGS+=("--registries" "$registry")
done

[ "$DRY_RUN" = "true" ] && CMD_ARGS+=("--dry-run")

# Add RPC_URL if set
[ -n "$RPC_URL" ] && CMD_ARGS+=("--rpc-url" "$RPC_URL")

info "Starting liquidator..."
echo ""
exec "$BINARY_PATH" "${CMD_ARGS[@]}"
