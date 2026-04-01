#!/bin/bash
# USAGE:
#   cp .env.example .env
#   # Edit .env: set SIGNER_ACCOUNT_ID and SIGNER_KEY
#   ./scripts/run-mainnet.sh

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
LIQUIDATION_SCAN_INTERVAL="${LIQUIDATION_SCAN_INTERVAL:-600}"
REGISTRY_REFRESH_INTERVAL="${REGISTRY_REFRESH_INTERVAL:-3600}"
CONCURRENCY="${CONCURRENCY:-10}"
PARTIAL_LIQUIDATION_PERCENTAGE="${PARTIAL_LIQUIDATION_PERCENTAGE}"
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
REF_CONTRACT="${REF_CONTRACT:-v2.ref-finance.near}"  # Mainnet default

# Market filtering configuration
ALLOWED_COLLATERAL_ASSETS="${ALLOWED_COLLATERAL_ASSETS}"
IGNORED_COLLATERAL_ASSETS="${IGNORED_COLLATERAL_ASSETS}"
IGNORED_MARKETS="${IGNORED_MARKETS}"

# Oracle price update configuration
PYTH_HERMES_URL="${PYTH_HERMES_URL:-https://hermes.pyth.network}"
REDSTONE_GATEWAY_URL="${REDSTONE_GATEWAY_URL:-https://oracle-gateway-1.a.redstone.vip}"

# Build binary
PROJECT_ROOT="$SCRIPT_DIR/../../.."
BINARY_PATH="$PROJECT_ROOT/target/debug/liquidator"

info "Building liquidator..."
cd "$PROJECT_ROOT"
cargo build -p templar-liquidator --bin liquidator
if [ ! -f "$BINARY_PATH" ]; then
    error "Build failed"
    exit 1
fi

# Print configuration
echo ""
info "Templar Liquidator - Mainnet (Inventory-Based)"
echo ""
echo "  Network:              $NETWORK"
echo "  Account:              $SIGNER_ACCOUNT_ID"
echo "  Registries:           $REGISTRIES"

# Show liquidation strategy
if [ -n "$FIXED_LIQUIDATION_AMOUNT_USD" ]; then
    echo "  Liquidation Strategy: Fixed Amount ($FIXED_LIQUIDATION_AMOUNT_USD USD)"
elif [ -n "$PARTIAL_LIQUIDATION_PERCENTAGE" ]; then
    echo "  Liquidation Strategy: Percentage ($PARTIAL_LIQUIDATION_PERCENTAGE%)"
else
    echo "  Liquidation Strategy: Percentage (100% - default)"
fi

echo "  Min Profit:           ${MIN_PROFIT_BPS} bps"
echo "  Dry Run:              $DRY_RUN"

# Show market filtering if configured
if [ -n "$ALLOWED_COLLATERAL_ASSETS" ]; then
    echo "  Allowed Assets:       $ALLOWED_COLLATERAL_ASSETS"
fi
if [ -n "$IGNORED_COLLATERAL_ASSETS" ]; then
    echo "  Ignored Assets:       $IGNORED_COLLATERAL_ASSETS"
fi
if [ -n "$IGNORED_MARKETS" ]; then
    echo "  Ignored Markets:      $IGNORED_MARKETS"
fi

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
    "--liquidation-scan-interval" "$LIQUIDATION_SCAN_INTERVAL"
    "--registry-refresh-interval" "$REGISTRY_REFRESH_INTERVAL"
    "--concurrency" "$CONCURRENCY"
    "--min-profit-bps" "$MIN_PROFIT_BPS"
    "--transaction-timeout" "$TRANSACTION_TIMEOUT"
)

for registry in $REGISTRIES; do
    CMD_ARGS+=("--registries" "$registry")
done

[ "$DRY_RUN" = "true" ] && CMD_ARGS+=("--dry-run")

# Add NEAR_RPC_URL if set
[ -n "$NEAR_RPC_URL" ] && CMD_ARGS+=("--near-rpc-url" "$NEAR_RPC_URL")
[ -n "$NEAR_API_KEY" ] && CMD_ARGS+=("--near-api-key" "$NEAR_API_KEY")

# Add liquidation strategy arguments (mutually exclusive)
[ -n "$PARTIAL_LIQUIDATION_PERCENTAGE" ] && CMD_ARGS+=("--partial-percentage" "$PARTIAL_LIQUIDATION_PERCENTAGE")
[ -n "$FIXED_LIQUIDATION_AMOUNT_USD" ] && CMD_ARGS+=("--fixed-liquidation-amount-usd" "$FIXED_LIQUIDATION_AMOUNT_USD")

# Add loop liquidation arguments
[ "$LOOP_LIQUIDATION" = "true" ] && CMD_ARGS+=("--loop-liquidation")
[ -n "$MAX_LOOP_ITERATIONS" ] && CMD_ARGS+=("--max-loop-iterations" "$MAX_LOOP_ITERATIONS")

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

if [ -n "$IGNORED_MARKETS" ]; then
    IFS=',' read -ra MARKETS <<< "$IGNORED_MARKETS"
    for market in "${MARKETS[@]}"; do
        CMD_ARGS+=("--ignored-markets" "$market")
    done
fi

# Add oracle price update arguments
CMD_ARGS+=("--hermes-url" "$PYTH_HERMES_URL")
CMD_ARGS+=("--redstone-gateway-url" "$REDSTONE_GATEWAY_URL")

# Add Telegram notification arguments (use = syntax because chat IDs start with -)
[ -n "$TELEGRAM_BOT_TOKEN" ] && CMD_ARGS+=("--telegram-bot-token=$TELEGRAM_BOT_TOKEN")
[ -n "$TELEGRAM_CHAT_ID" ] && CMD_ARGS+=("--telegram-chat-id=$TELEGRAM_CHAT_ID")
[ -n "$TELEGRAM_THREAD_ID" ] && CMD_ARGS+=("--telegram-thread-id=$TELEGRAM_THREAD_ID")

info "Starting liquidator..."
echo ""
exec "$BINARY_PATH" "${CMD_ARGS[@]}"
