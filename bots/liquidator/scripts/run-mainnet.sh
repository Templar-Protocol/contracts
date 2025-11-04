#!/bin/bash
# SPDX-License-Identifier: MIT
#
# Run Templar liquidator on mainnet.
# Default settings run in DRY RUN mode (DRY_RUN=true).
#
# USAGE:
#   cp .env.example .env
#   # Edit .env: set SIGNER_ACCOUNT_ID and SIGNER_KEY
#   ./scripts/run-mainnet.sh
#
# CONFIGURATION:
#   All settings loaded from .env file. Required variables:
#   - SIGNER_ACCOUNT_ID: Your NEAR account (e.g., liquidator.near)
#   - SIGNER_KEY: Account private key (ed25519:...)
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
if [ -z "$SIGNER_ACCOUNT_ID" ]; then
    error "SIGNER_ACCOUNT_ID not set"
    echo "  Set in .env or: export SIGNER_ACCOUNT_ID=\"your-account.near\""
    exit 1
fi

if [ -z "$SIGNER_KEY" ]; then
    error "SIGNER_KEY not set"
    echo "  Set in .env or: export SIGNER_KEY=\"ed25519:...\""
    exit 1
fi

# Configuration with defaults (see .env.example for all options)
NETWORK="${NETWORK:-mainnet}"
REGISTRIES="${REGISTRY_ACCOUNT_IDS:-v1.tmplr.near}"
LIQUIDATION_STRATEGY="${LIQUIDATION_STRATEGY:-partial}"
LIQUIDATION_SCAN_INTERVAL="${LIQUIDATION_SCAN_INTERVAL:-600}"
REGISTRY_REFRESH_INTERVAL="${REGISTRY_REFRESH_INTERVAL:-3600}"
CONCURRENCY="${CONCURRENCY:-10}"
PARTIAL_PERCENTAGE="${PARTIAL_PERCENTAGE:-50}"
TRANSACTION_TIMEOUT="${TRANSACTION_TIMEOUT:-60}"
MAX_GAS_PERCENTAGE="${MAX_GAS_PERCENTAGE:-10}"
MIN_PROFIT_BPS="${MIN_PROFIT_BPS:-50}"
DRY_RUN="${DRY_RUN:-true}"

# Collateral strategy configuration
COLLATERAL_STRATEGY="${COLLATERAL_STRATEGY:-hold}"
PRIMARY_ASSET="${PRIMARY_ASSET}"

# Swap provider configuration (both providers will be initialized automatically)
ONECLICK_API_TOKEN="${ONECLICK_API_TOKEN}"
REF_CONTRACT="${REF_CONTRACT:-v2.ref-finance.near}"  # Mainnet default

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
info "Templar Liquidator - Mainnet (Inventory-Based)"
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
    "--signer-account" "$SIGNER_ACCOUNT_ID"
    "--signer-key" "$SIGNER_KEY"
    "--liquidation-strategy" "$LIQUIDATION_STRATEGY"
    "--liquidation-scan-interval" "$LIQUIDATION_SCAN_INTERVAL"
    "--registry-refresh-interval" "$REGISTRY_REFRESH_INTERVAL"
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

# Add collateral strategy arguments
CMD_ARGS+=("--collateral-strategy" "$COLLATERAL_STRATEGY")
[ -n "$PRIMARY_ASSET" ] && CMD_ARGS+=("--primary-asset" "$PRIMARY_ASSET")
[ -n "$ONECLICK_API_TOKEN" ] && CMD_ARGS+=("--oneclick-api-token" "$ONECLICK_API_TOKEN")
[ -n "$REF_CONTRACT" ] && CMD_ARGS+=("--ref-contract" "$REF_CONTRACT")

info "Starting liquidator..."
echo ""
exec "$BINARY_PATH" "${CMD_ARGS[@]}"
