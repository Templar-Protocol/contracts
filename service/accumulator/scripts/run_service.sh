#!/bin/bash
#
# Templar Accumulator - Startup Script
#
# Usage:
#   ./scripts/run_service.sh [OPTIONS] [mainnet|testnet]
#
# Examples:
#   ./scripts/run_service.sh
#   ./scripts/run_service.sh testnet
#   ./scripts/run_service.sh --skip-build

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_header() {
    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}  Templar Accumulator Service${NC}"
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

usage() {
    echo "Usage: ./scripts/run_service.sh [OPTIONS] [mainnet|testnet]"
    echo ""
    echo "Options:"
    echo "  --skip-build    Skip building the binary if it exists"
    echo "  --help, -h      Show this help message"
    echo ""
    echo "Arguments:"
    echo "  mainnet|testnet Override NETWORK from .env file"
    echo ""
    echo "Environment variables are loaded from .env"
    echo "See .env.example for configuration options"
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
    # shellcheck disable=SC1091
    source .env
    set +a
}

set_defaults() {
    export NETWORK="${NETWORK:-mainnet}"
    if [ -z "${REGISTRIES_ACCOUNT_IDS:-}" ]; then
        if [ "$NETWORK" = "testnet" ]; then
            export REGISTRIES_ACCOUNT_IDS="templar-registry.testnet"
        else
            export REGISTRIES_ACCOUNT_IDS="v1.tmplr.near"
        fi
    fi
    export TIMEOUT="${TIMEOUT:-60}"
    export INTERVAL="${INTERVAL:-600}"
    export STATIC_INTERVAL="${STATIC_INTERVAL:-86400}"
    export REGISTRY_REFRESH_INTERVAL="${REGISTRY_REFRESH_INTERVAL:-3600}"
    export CONCURRENCY="${CONCURRENCY:-4}"
    export RUST_LOG="${RUST_LOG:-info,templar_accumulator=debug}"
    export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
}

validate_config() {
    print_info "Validating configuration..."

    if [ -z "${SIGNER_ACCOUNT_ID:-}" ]; then
        print_error "SIGNER_ACCOUNT_ID is required"
        exit 1
    fi

    if [ -z "${SIGNER_KEY:-}" ] && [ -z "${SIGNER_KEY_FILE:-}" ]; then
        print_error "Set either SIGNER_KEY or SIGNER_KEY_FILE"
        exit 1
    fi

    if [ -n "${SIGNER_KEY_FILE:-}" ] && [ ! -f "$SIGNER_KEY_FILE" ]; then
        print_error "SIGNER_KEY_FILE does not exist: $SIGNER_KEY_FILE"
        exit 1
    fi

    if [ -n "${SIGNER_KEY:-}" ] && [[ "$SIGNER_KEY" == *"YOUR_PRIVATE_KEY"* ]]; then
        print_error "Please replace placeholder SIGNER_KEY in .env"
        exit 1
    fi

    print_info "Configuration validated"
}

check_binary() {
    local binary_path="../../target/release/accumulator"

    if [ ! -f "$binary_path" ]; then
        print_warn "Binary not found at $binary_path"
        print_info "Building accumulator..."
        cargo build --release -p templar-accumulator --bin accumulator
    fi

    if [ ! -x "$binary_path" ]; then
        print_error "Binary at $binary_path is not executable"
        exit 1
    fi

    BINARY_PATH="$binary_path"
}

print_config() {
    echo ""
    echo -e "${BLUE}Configuration:${NC}"
    echo "  Network: ${NETWORK}"
    echo "  Registries: ${REGISTRIES_ACCOUNT_IDS}"
    echo "  Signer Account: ${SIGNER_ACCOUNT_ID}"
    if [ -n "${SIGNER_KEY_FILE:-}" ]; then
        echo "  Signer Key Source: file (${SIGNER_KEY_FILE})"
    else
        echo "  Signer Key Source: env"
    fi
    echo "  Timeout: ${TIMEOUT}s"
    echo "  Interval: ${INTERVAL}s"
    echo "  Static Interval: ${STATIC_INTERVAL}s"
    echo "  Registry Refresh: ${REGISTRY_REFRESH_INTERVAL}s"
    echo "  Concurrency: ${CONCURRENCY}"
    echo "  Log Level: ${RUST_LOG}"
    echo ""
}

check_running_instances() {
    if pgrep -f "(/|^)accumulator( |$)" > /dev/null; then
        print_warn "Another accumulator instance appears to be running"
        read -p "Kill existing instances? (y/N) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            pkill -f "(/|^)accumulator( |$)" || true
            sleep 2
        else
            print_info "Aborting..."
            exit 1
        fi
    fi
}

print_header

SKIP_BUILD=false
SHOW_HELP=false
NETWORK_ARG=""

while [[ $# -gt 0 ]]; do
    case "$1" in
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
    usage
    exit 0
fi

check_env_file
load_env

if [ -n "$NETWORK_ARG" ]; then
    export NETWORK="$NETWORK_ARG"
    print_info "Network override: $NETWORK"
fi

set_defaults
validate_config

if [ "$SKIP_BUILD" = false ]; then
    check_binary
else
    BINARY_PATH="../../target/release/accumulator"
    if [ ! -f "$BINARY_PATH" ]; then
        print_error "Binary not found and --skip-build specified"
        exit 1
    fi
fi

print_config
check_running_instances

print_info "Starting Accumulator Service..."
echo ""
exec "$BINARY_PATH"
