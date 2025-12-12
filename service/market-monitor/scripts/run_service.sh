#!/bin/bash
#
# Templar Market Monitor - Startup Script
#
# This script starts the market monitor service with proper environment configuration.
# It loads configuration from .env file and supports both development and production modes.
#
# Usage:
#   ./scripts/run_service.sh [OPTIONS]
#
# Options:
#   --skip-build    Skip building the binary
#   --once          Run once and exit (overrides RUN_ONCE env var)
#   --help          Show this help message
#
# Example:
#   ./scripts/run_service.sh                    # Run with .env config
#   ./scripts/run_service.sh --once             # Run single scan
#   ./scripts/run_service.sh --skip-build       # Skip building binary
#

set -e  # Exit on error

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(dirname "$(dirname "$PROJECT_DIR")")"

# Change to project directory
cd "$PROJECT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ============================================
# Helper Functions
# ============================================

print_header() {
    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}  Templar Market Monitor Service${NC}"
    echo -e "${BLUE}================================================${NC}"
    echo ""
}

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

show_help() {
    cat << EOF
Templar Market Monitor - Startup Script

Usage:
  ./scripts/run_service.sh [OPTIONS]

Options:
  --skip-build    Skip building the binary
  --once          Run once and exit (overrides RUN_ONCE env var)
  --help          Show this help message

Configuration:
  Configuration is loaded from .env file in the service directory.
  Copy .env.example to .env and edit as needed.

  Key variables:
    REGISTRY_ACCOUNT_IDS       - Comma-separated list of registry contracts
    TELEGRAM_BOT_TOKEN         - Telegram bot token (leave empty to skip sending)
    TELEGRAM_CHANNEL_ID        - Telegram channel ID
    TELEGRAM_THREAD_ID         - Telegram thread/topic ID (optional)
    SCAN_TIME                  - Scan time (*/N for interval or HH:MM for daily UTC)
    AT_RISK_THRESHOLD_PERCENT  - At risk threshold percentage above MCR
    MIN_POSITION_SIZE_USD      - Minimum position size to report

Examples:
  ./scripts/run_service.sh                    # Run continuous monitoring
  ./scripts/run_service.sh --once             # Run single scan
  ./scripts/run_service.sh --skip-build       # Use existing binary

EOF
}

# ============================================
# Parse Arguments
# ============================================

SKIP_BUILD=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --help)
            show_help
            exit 0
            ;;
        *)
            print_error "Unknown option: $1"
            echo ""
            show_help
            exit 1
            ;;
    esac
done

# ============================================
# Load Environment
# ============================================

print_header

ENV_FILE="$PROJECT_DIR/.env"
if [ -f "$ENV_FILE" ]; then
    print_info "Loading configuration from .env"
    # shellcheck disable=SC1090
    source "$ENV_FILE"
else
    print_warn "No .env file found. Using environment variables."
    print_warn "Copy .env.example to .env to customize configuration."
fi

# ============================================
# Configuration with Defaults
# ============================================

NETWORK="${NETWORK:-mainnet}"
REGISTRY_ACCOUNT_IDS="${REGISTRY_ACCOUNT_IDS:-v1.tmplr.near}"
SCAN_TIME="${SCAN_TIME:-00:00}"
AT_RISK_THRESHOLD_PERCENT="${AT_RISK_THRESHOLD_PERCENT:-10}"
MIN_POSITION_SIZE_USD="${MIN_POSITION_SIZE_USD:-1000}"

TELEGRAM_BOT_TOKEN="${TELEGRAM_BOT_TOKEN:-}"
TELEGRAM_CHANNEL_ID="${TELEGRAM_CHANNEL_ID:-}"
TELEGRAM_THREAD_ID="${TELEGRAM_THREAD_ID:-}"

# ============================================
# Validate Configuration
# ============================================

print_info "Validating configuration..."

if [ -z "$REGISTRY_ACCOUNT_IDS" ]; then
    print_error "REGISTRY_ACCOUNT_IDS not set"
    echo "  Set in .env or: export REGISTRY_ACCOUNT_IDS=\"v1.tmplr.near\""
    exit 1
fi

if [ -z "$TELEGRAM_BOT_TOKEN" ]; then
    print_warn "TELEGRAM_BOT_TOKEN not set - reports will not be sent to Telegram"
    print_warn "Set TELEGRAM_BOT_TOKEN in .env to enable Telegram notifications"
fi

if [ -z "$TELEGRAM_CHANNEL_ID" ] && [ -n "$TELEGRAM_BOT_TOKEN" ]; then
    print_warn "TELEGRAM_CHANNEL_ID not set but token is provided"
    print_warn "Set TELEGRAM_CHANNEL_ID in .env to send to a specific channel"
fi

# ============================================
# Build Binary
# ============================================

BINARY_PATH="$REPO_ROOT/target/release/market-monitor"

if [ "$SKIP_BUILD" = false ]; then
    print_info "Building market-monitor binary..."
    cd "$REPO_ROOT"
    cargo build --release -p templar-market-monitor
    
    if [ ! -f "$BINARY_PATH" ]; then
        print_error "Build failed - binary not found at $BINARY_PATH"
        exit 1
    fi
    
    print_info "Build successful"
    cd "$PROJECT_DIR"
else
    print_info "Skipping build (--skip-build flag set)"
    
    if [ ! -f "$BINARY_PATH" ]; then
        print_error "Binary not found at $BINARY_PATH"
        print_error "Run without --skip-build to build it first"
        exit 1
    fi
fi

# ============================================
# Display Configuration
# ============================================

echo ""
print_info "Configuration:"
echo ""
echo "  Network:              $NETWORK"
echo "  Registries:           $REGISTRY_ACCOUNT_IDS"
echo "  Scan Time:            $SCAN_TIME UTC"
echo "  At Risk Threshold:    ${AT_RISK_THRESHOLD_PERCENT}%"
echo "  Min Position Size:    \$${MIN_POSITION_SIZE_USD}"

if [ -n "$TELEGRAM_BOT_TOKEN" ]; then
    echo "  Telegram:             Enabled"
    if [ -n "$TELEGRAM_CHANNEL_ID" ]; then
        echo "  Channel ID:           $TELEGRAM_CHANNEL_ID"
    fi
    if [ -n "$TELEGRAM_THREAD_ID" ]; then
        echo "  Thread ID:            $TELEGRAM_THREAD_ID"
    fi
else
    echo "  Telegram:             Disabled (no token)"
fi

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

# ============================================
# Run Service
# ============================================

print_info "Starting continuous monitoring..."
if [[ "$SCAN_TIME" == *"*/"* ]]; then
    INTERVAL="${SCAN_TIME#*/}"
    print_info "Service will scan every $INTERVAL minutes (starting immediately)"
else
    print_info "Service will scan daily at $SCAN_TIME UTC"
fi
print_info "Press Ctrl+C to stop"

echo ""

# Execute the binary
exec "$BINARY_PATH"
