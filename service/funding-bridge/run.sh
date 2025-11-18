#!/bin/bash
# Templar Funding Bridge - Startup Script

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

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
    if [ -z "$NEAR_ENABLED" ] || [ "$NEAR_ENABLED" != "true" ]; then
        print_error "NEAR_ENABLED must be set to 'true'"
        exit 1
    fi

    if [ -z "$NEAR_TREASURY_ACCOUNT" ]; then
        print_error "NEAR_TREASURY_ACCOUNT is required"
        exit 1
    fi

    if [ -z "$NEAR_SIGNER_KEY" ]; then
        print_error "NEAR_SIGNER_KEY is required"
        exit 1
    fi

    # Check for placeholder values
    if [[ "$NEAR_SIGNER_KEY" == *"YOUR_PRIVATE_KEY_HERE"* ]]; then
        print_error "Please replace placeholder values in .env"
        exit 1
    fi

    # Warn about optional features
    if [ -z "$ETH_PRIVATE_KEY" ]; then
        print_warn "ETH_PRIVATE_KEY not set - EVM deposits disabled"
    fi

    if [ -z "$SOLANA_PRIVATE_KEY" ]; then
        print_warn "SOLANA_PRIVATE_KEY not set - Solana deposits disabled"
    fi

    # Check dry run mode
    if [ "$DRY_RUN" = "true" ]; then
        print_warn "Running in DRY RUN mode - transactions will be simulated"
    fi

    print_info "Configuration validated"
}

build_args() {
    ARGS=(
        "--port" "${PORT:-3000}"
        "--network" "${NETWORK:-mainnet}"
    )

    if [ -n "$NEAR_RPC_URL" ]; then
        ARGS+=("--near-rpc-url" "$NEAR_RPC_URL")
    fi

    if [ -n "$BRIDGE_API_URL" ]; then
        ARGS+=("--bridge-api-url" "$BRIDGE_API_URL")
    fi

    if [ "$DRY_RUN" = "true" ]; then
        ARGS+=("--dry-run")
    fi

    # NEAR configuration
    ARGS+=("--near-enabled")
    ARGS+=("--near-treasury-account" "$NEAR_TREASURY_ACCOUNT")
    ARGS+=("--near-signer-key" "$NEAR_SIGNER_KEY")

    if [ -n "$NEAR_PRIORITY" ]; then
        ARGS+=("--near-priority" "$NEAR_PRIORITY")
    fi
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
    echo ""
}

# ============================================
# Main Execution
# ============================================

print_header

# Parse command line arguments
SKIP_BUILD=false
SHOW_HELP=false

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
        *)
            print_error "Unknown option: $1"
            SHOW_HELP=true
            shift
            ;;
    esac
done

if [ "$SHOW_HELP" = true ]; then
    echo "Usage: ./run.sh [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --skip-build    Skip building the binary if it exists"
    echo "  --help, -h      Show this help message"
    echo ""
    echo "Environment variables are loaded from .env file"
    echo "See .env.example for configuration options"
    exit 0
fi

# Step 1: Check environment file
check_env_file

# Step 2: Load environment variables
load_env

# Step 3: Validate configuration
validate_config

# Step 4: Check/build binary
if [ "$SKIP_BUILD" = false ]; then
    check_binary
else
    BINARY_PATH="../../target/release/funding-bridge"
    if [ ! -f "$BINARY_PATH" ]; then
        print_error "Binary not found and --skip-build specified"
        exit 1
    fi
fi

# Step 5: Build command arguments
build_args

# Step 6: Print configuration
print_config

# Step 7: Start the service
print_info "Starting Funding Bridge Service..."
echo ""

# Run with explicit environment variables for logging
exec env \
    RUST_LOG="${RUST_LOG:-info}" \
    RUST_BACKTRACE="${RUST_BACKTRACE:-1}" \
    "$BINARY_PATH" "${ARGS[@]}"
