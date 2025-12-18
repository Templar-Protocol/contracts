#!/bin/bash
# USAGE:
#   cp .env.example .env
#   # Edit .env: set SIGNER_ACCOUNT_ID and SIGNER_KEY
#   ./scripts/run-testnet.sh

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
CONCURRENCY="${CONCURRENCY:-10}"
PARTIAL_LIQUIDATION_PERCENTAGE="${PARTIAL_LIQUIDATION_PERCENTAGE:-50}"
FIXED_LIQUIDATION_AMOUNT_USD="${FIXED_LIQUIDATION_AMOUNT_USD}"
LOOP_LIQUIDATION="${LOOP_LIQUIDATION:-false}"
MAX_LOOP_ITERATIONS="${MAX_LOOP_ITERATIONS:-10}"
TRANSACTION_TIMEOUT="${TRANSACTION_TIMEOUT:-60}"
MIN_PROFIT_BPS="${MIN_PROFIT_BPS:-50}"
DRY_RUN="${DRY_RUN:-true}"

# Collateral strategy configuration
COLLATERAL_STRATEGY="${COLLATERAL_STRATEGY:-hold}"

# Swap provider configuration (both providers will be initialized automatically)
ONECLICK_API_TOKEN="${ONECLICK_API_TOKEN}"
REF_CONTRACT="${REF_CONTRACT:-v2.ref-dev.testnet}"  # Testnet default

# Market filtering configuration
ALLOWED_COLLATERAL_ASSETS="${ALLOWED_COLLATERAL_ASSETS}"
IGNORED_COLLATERAL_ASSETS="${IGNORED_COLLATERAL_ASSETS}"

# Oracle price update configuration
PYTH_HERMES_URL="${PYTH_HERMES_URL:-https://hermes-beta.pyth.network}"
AUTO_UPDATE_PRICES="${AUTO_UPDATE_PRICES:-false}"

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

# Show market filtering if configured
if [ -n "$ALLOWED_COLLATERAL_ASSETS" ]; then
    echo "  Allowed Assets:       $ALLOWED_COLLATERAL_ASSETS"
fi
if [ -n "$IGNORED_COLLATERAL_ASSETS" ]; then
    echo "  Ignored Assets:       $IGNORED_COLLATERAL_ASSETS"
fi

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
    "--concurrency" "$CONCURRENCY"
    "--partial-percentage" "$PARTIAL_LIQUIDATION_PERCENTAGE"
    "--min-profit-bps" "$MIN_PROFIT_BPS"
    "--transaction-timeout" "$TRANSACTION_TIMEOUT"
)

for registry in $REGISTRIES; do
    CMD_ARGS+=("--registries" "$registry")
done

[ "$DRY_RUN" = "true" ] && CMD_ARGS+=("--dry-run")

# Add RPC_URL if set
[ -n "$RPC_URL" ] && CMD_ARGS+=("--rpc-url" "$RPC_URL")

# Add loop liquidation arguments
[ "$LOOP_LIQUIDATION" = "true" ] && CMD_ARGS+=("--loop-liquidation")
[ -n "$MAX_LOOP_ITERATIONS" ] && CMD_ARGS+=("--max-loop-iterations" "$MAX_LOOP_ITERATIONS")
[ -n "$FIXED_LIQUIDATION_AMOUNT_USD" ] && CMD_ARGS+=("--fixed-liquidation-amount-usd" "$FIXED_LIQUIDATION_AMOUNT_USD")

# Add collateral strategy arguments
CMD_ARGS+=("--collateral-strategy" "$COLLATERAL_STRATEGY")
[ -n "$ONECLICK_API_TOKEN" ] && CMD_ARGS+=("--oneclick-api-token" "$ONECLICK_API_TOKEN")
[ -n "$REF_CONTRACT" ] && CMD_ARGS+=("--ref-contract" "$REF_CONTRACT")

# Add market filtering arguments
if [ -n "$ALLOWED_COLLATERAL_ASSETS" ]; then
    IFS=',' read -ra ASSETS <<< "$ALLOWED_COLLATERAL_ASSETS"
    for asset in "${ASSETS[@]}"; do
        CMD_ARGS+=("--allowed-collateral-assets" "$asset")
    done
fi

if [ -n "$IGNORED_COLLATERAL_ASSETS" ]; then
    IFS=',' read -ra ASSETS <<< "$IGNORED_COLLATERAL_ASSETS"
    for asset in "${ASSETS[@]}"; do
        CMD_ARGS+=("--ignored-collateral-assets" "$asset")
    done
fi

# Add oracle price update arguments
CMD_ARGS+=("--hermes-url" "$PYTH_HERMES_URL")
[ "$AUTO_UPDATE_PRICES" = "true" ] && CMD_ARGS+=("--auto-update-prices")

info "Starting liquidator..."
echo ""
exec "$BINARY_PATH" "${CMD_ARGS[@]}"
