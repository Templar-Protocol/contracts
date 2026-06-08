#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

show_usage() {
    cat <<'EOF'
Usage:
  ./scripts/complete_hot_deposit.sh <nonce> <sender_id> <token_id> <amount> [receiver_id]

Arguments:
  nonce        HOT/Stellar deposit nonce
  sender_id    Stellar sender account (G...)
  token_id     HOT token id (for example 1100_C...)
  amount       Amount as integer string in token base units
  receiver_id  NEAR receiver account. Defaults to HOT_RELAYER_NEAR_RECEIVER from .env

Environment:
  SERVICE_URL                  Default: http://localhost:3001
  HOT_RELAYER_AUTH_TOKEN       Required bearer token
  HOT_RELAYER_NEAR_RECEIVER    Required when receiver_id is not passed
  HOT_RELAYER_TOKEN_ID         Expected HOT token id configured in the service
  HOT_RELAYER_CHAIN_ID         Default: 1100

Example:
  ./scripts/complete_hot_deposit.sh \
    123 \
    GABCDEFG... \
    1100_CABCDEFG... \
    1000 \
    carrion256.near
EOF
}

check_env_file() {
    if [ ! -f "$PROJECT_DIR/.env" ]; then
        print_error "Missing $PROJECT_DIR/.env"
        print_warn "Create it from .env.example and set HOT relayer variables first"
        exit 1
    fi
}

load_env() {
    set -a
    source "$PROJECT_DIR/.env"
    set +a
}

require_health() {
    if ! curl -fsS "$SERVICE_URL/health" >/dev/null; then
        print_error "Funding Bridge service is not reachable at $SERVICE_URL"
        print_warn "Start it first with ./scripts/run_service.sh"
        exit 1
    fi
}

build_auth_args() {
    AUTH_ARGS=(-H "Authorization: Bearer $HOT_RELAYER_AUTH_TOKEN")
}

check_required_config() {
    if [ -z "${HOT_MPC_API_URL:-}" ]; then
        print_error "HOT_MPC_API_URL is required in .env"
        exit 1
    fi

    if [ -z "${HOT_RELAYER_NEAR_RECEIVER:-}" ]; then
        print_error "HOT_RELAYER_NEAR_RECEIVER is required in .env"
        exit 1
    fi

    if [ -z "${HOT_RELAYER_TOKEN_ID:-}" ]; then
        print_error "HOT_RELAYER_TOKEN_ID is required in .env"
        exit 1
    fi

    if [ -z "${HOT_RELAYER_AUTH_TOKEN:-}" ]; then
        print_error "HOT_RELAYER_AUTH_TOKEN is required in .env"
        exit 1
    fi
}

NONCE="${1:-}"
SENDER_ID="${2:-}"
TOKEN_ID="${3:-}"
AMOUNT="${4:-}"
RECEIVER_ID="${5:-${HOT_RELAYER_NEAR_RECEIVER:-}}"
HOT_RELAYER_CHAIN_ID="${HOT_RELAYER_CHAIN_ID:-1100}"
SERVICE_URL="${SERVICE_URL:-http://localhost:3001}"

if [ -z "$NONCE" ] || [ -z "$SENDER_ID" ] || [ -z "$TOKEN_ID" ] || [ -z "$AMOUNT" ]; then
    show_usage
    exit 1
fi

check_env_file
load_env
RECEIVER_ID="${5:-${HOT_RELAYER_NEAR_RECEIVER:-}}"
HOT_RELAYER_CHAIN_ID="${HOT_RELAYER_CHAIN_ID:-1100}"

if [ -z "$RECEIVER_ID" ]; then
    print_error "receiver_id is required either as arg 5 or HOT_RELAYER_NEAR_RECEIVER in .env"
    exit 1
fi

check_required_config
build_auth_args
require_health

JSON_PAYLOAD="$(python3 - <<PY
import json
payload = {
    "event": {
        "chain_id": int(${HOT_RELAYER_CHAIN_ID}),
        "nonce": ${NONCE@Q},
        "sender_id": ${SENDER_ID@Q},
        "receiver_id": ${RECEIVER_ID@Q},
        "token_id": ${TOKEN_ID@Q},
        "amount": ${AMOUNT@Q},
    }
}
print(json.dumps(payload))
PY
)"

print_info "Submitting one-shot HOT deposit completion request"
echo -e "${BLUE}Service:${NC} $SERVICE_URL"
echo -e "${BLUE}Chain ID:${NC} $HOT_RELAYER_CHAIN_ID"
echo -e "${BLUE}Nonce:${NC} $NONCE"
echo -e "${BLUE}Sender:${NC} $SENDER_ID"
echo -e "${BLUE}Receiver:${NC} $RECEIVER_ID"
echo -e "${BLUE}Token:${NC} $TOKEN_ID"
echo -e "${BLUE}Amount:${NC} $AMOUNT"
echo ""

curl -fsS -X POST "$SERVICE_URL/relay/deposit/complete" \
    "${AUTH_ARGS[@]}" \
    -H "Content-Type: application/json" \
    -d "$JSON_PAYLOAD" | python3 -m json.tool
