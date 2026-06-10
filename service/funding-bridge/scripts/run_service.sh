#!/bin/bash
#
# Templar Funding Bridge - Startup Script
#
# This script starts the funding bridge service with proper environment configuration.
# It loads configuration from .env file and supports both development and production modes.
#
# Usage:
#   ./scripts/run_service.sh [OPTIONS] [mainnet|testnet]
#
# Example:
#   ./scripts/run_service.sh                    # Run with .env config
#   ./scripts/run_service.sh mainnet            # Run on mainnet
#   ./scripts/run_service.sh --skip-build       # Skip building binary
#   ./scripts/run_service.sh --help             # Show help
#

set -e  # Exit on error

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

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
    echo -e "${BLUE}  Templar Funding Bridge Service${NC}"
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

check_env_file() {
    if [ ! -f .env ]; then
        print_warn ".env file not found"
        if [ -f .env.example ]; then
            print_info "Creating .env from .env.example"
            cp .env.example .env
            print_warn "Please edit .env with your configuration before running"
            exit 1
        else
            print_error "Neither .env nor .env.example found"
            exit 1
        fi
    fi
}

load_env() {
    print_info "Loading environment from .env"
    set -a
    source .env
    set +a
}

check_binary() {
    local binary_path="../../target/release/funding-bridge"

    if [ ! -f "$binary_path" ]; then
        print_warn "Binary not found at $binary_path"
        print_info "Building funding-bridge..."

        # Check if we need to build with features
        local features=""
        if [ -n "$ETH_PRIVATE_KEY" ]; then
            features="ethereum"
        fi
        if [ -n "$SOLANA_PRIVATE_KEY" ]; then
            if [ -n "$features" ]; then
                features="$features,solana"
            else
                features="solana"
            fi
        fi

        if [ -n "$features" ]; then
            print_info "Building with features: $features"
            cargo build --release --features "$features" -p templar-funding-bridge
        else
            cargo build --release -p templar-funding-bridge
        fi
    fi

    if [ ! -x "$binary_path" ]; then
        print_error "Binary at $binary_path is not executable"
        exit 1
    fi

    BINARY_PATH="$binary_path"
}

validate_config() {
    print_info "Validating configuration..."

    # Check required variables
    if [ -z "$NEAR_TREASURY_ACCOUNT" ]; then
        print_error "NEAR_TREASURY_ACCOUNT is required"
        exit 1
    fi

    if [ -z "$NEAR_TREASURY_KEY" ]; then
        print_error "NEAR_TREASURY_KEY is required"
        exit 1
    fi

    # Check for placeholder values
    if [[ "$NEAR_TREASURY_KEY" == *"YOUR_PRIVATE_KEY_HERE"* ]]; then
        print_error "Please replace placeholder values in .env"
        exit 1
    fi

    # Check dry run mode
    if [ "$DRY_RUN" = "true" ]; then
        print_warn "Running in DRY RUN mode - transactions will be simulated"
    fi

    print_info "Configuration validated"
}

print_config() {
    echo ""
    echo -e "${BLUE}Configuration:${NC}"
    echo "  Network: ${NETWORK:-mainnet}"
    echo "  Port: ${PORT:-3000}"
    echo "  Treasury: $NEAR_TREASURY_ACCOUNT"
    echo "  Dry Run: ${DRY_RUN:-false}"
    echo "  Log Level: ${RUST_LOG:-info}"

    if [ -n "$ETH_PRIVATE_KEY" ]; then
        echo "  EVM Deposits: Enabled"
    else
        echo "  EVM Deposits: Disabled"
    fi

    if [ -n "$SOLANA_PRIVATE_KEY" ]; then
        echo "  Solana Deposits: Enabled"
    else
        echo "  Solana Deposits: Disabled"
    fi

    if [ -n "$STELLAR_SECRET_KEY" ]; then
        echo "  Stellar Deposits: Enabled"
    else
        echo "  Stellar Deposits: Disabled"
    fi

    if [ -n "$NEAR_ACCOUNT" ] && [ -n "$NEAR_KEY" ]; then
        echo "  NEAR Deposits: Enabled"
    else
        echo "  NEAR Deposits: Disabled"
    fi
    echo ""
}

check_running_instances() {
    if pgrep -f "funding-bridge" > /dev/null; then
        print_warn "Another instance of funding-bridge is running"
        read -p "Kill existing instances? (y/N) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            print_info "Killing existing instances..."
            pkill -f "funding-bridge" || true
            sleep 2
        else
            print_info "Aborting..."
            exit 1
        fi
    fi
}

# ============================================
# Main Execution
# ============================================

print_header

# Parse command line arguments
SKIP_BUILD=false
SHOW_HELP=false
NETWORK_ARG=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --help|-h)
            SHOW_HELP=true
            shift
            ;;
        mainnet|testnet)
            NETWORK_ARG="$1"
            shift
            ;;
        *)
            print_error "Unknown option: $1"
            SHOW_HELP=true
            shift
            ;;
    esac
done

if [ "$SHOW_HELP" = true ]; then
    echo "Usage: ./scripts/run_service.sh [OPTIONS] [mainnet|testnet]"
    echo ""
    echo "Options:"
    echo "  --skip-build    Skip building the binary if it exists"
    echo "  --help, -h      Show this help message"
    echo ""
    echo "Arguments:"
    echo "  mainnet|testnet Override NETWORK from .env file"
    echo ""
    echo "Environment variables are loaded from .env file"
    echo "See .env.example for configuration options"
    exit 0
fi

# Step 1: Check environment file
check_env_file

# Step 2: Load environment variables
load_env

# Step 3: Override network if specified
if [ -n "$NETWORK_ARG" ]; then
    export NETWORK="$NETWORK_ARG"
    print_info "Network override: $NETWORK"
fi

# Step 4: Set default values
export NETWORK="${NETWORK:-mainnet}"
export DRY_RUN="${DRY_RUN:-false}"
export RUST_LOG="${RUST_LOG:-info,templar_funding_bridge=debug}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

# Step 5: Validate configuration
validate_config

# Step 6: Check/build binary
if [ "$SKIP_BUILD" = false ]; then
    check_binary
else
    BINARY_PATH="../../target/release/funding-bridge"
    if [ ! -f "$BINARY_PATH" ]; then
        print_error "Binary not found and --skip-build specified"
        exit 1
    fi
fi

# Step 7: Print configuration
print_config

# Step 8: Check for running instances
check_running_instances

# Step 9: Start the service
print_info "Starting Funding Bridge Service..."
echo ""

# Run the binary (environment variables already exported via set -a)
exec "$BINARY_PATH"
