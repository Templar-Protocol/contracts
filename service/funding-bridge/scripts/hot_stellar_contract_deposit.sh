#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

HOT_STELLAR_LOCKER_CONTRACT="${HOT_STELLAR_LOCKER_CONTRACT:-CCLWL5NYSV2WJQ3VBU44AMDHEVKEPA45N2QP2LL62O3JVKPGWWAQUVAG}"
STELLAR_RPC_URL="${STELLAR_RPC_URL:-https://mainnet.sorobanrpc.com}"
STELLAR_NETWORK_PASSPHRASE="${STELLAR_NETWORK_PASSPHRASE:-Public Global Stellar Network ; September 2015}"
TARGET_NEAR_ACCOUNT="${1:-carrion256.near}"
ASSET_KIND="${2:-native}"
AMOUNT="${3:-1000000}"

if [ ! -f "$PROJECT_DIR/.env" ]; then
  echo "Missing $PROJECT_DIR/.env" >&2
  exit 1
fi

set -a
source "$PROJECT_DIR/.env"
set +a

if [ -z "${STELLAR_SECRET_KEY:-}" ]; then
  echo "STELLAR_SECRET_KEY is required in $PROJECT_DIR/.env" >&2
  exit 1
fi

if [ "$ASSET_KIND" != "native" ]; then
  echo "Only native XLM is supported by this proof script right now" >&2
  exit 1
fi

SENDER_ACCOUNT="$(stellar keys address templar-hot-mainnet)"
TOKEN_CONTRACT="$(stellar contract id asset --asset native --rpc-url "$STELLAR_RPC_URL" --network-passphrase "$STELLAR_NETWORK_PASSPHRASE")"
CLIENT_TIMESTAMP="$(python3 - <<'PY'
import time
print(time.time_ns() - 20_000_000_000)
PY
)"
RECEIVER_HEX="$(python3 - <<PY
import hashlib, json
target = ${TARGET_NEAR_ACCOUNT@Q}
payload = json.dumps({"account_id": "intents.near", "msg": json.dumps({"receiver_id": target})})
print(hashlib.sha256(payload.encode()).hexdigest())
PY
)"

echo "HOT locker contract: $HOT_STELLAR_LOCKER_CONTRACT"
echo "Stellar sender: $SENDER_ACCOUNT"
echo "NEAR receiver: $TARGET_NEAR_ACCOUNT"
echo "Receiver hash: $RECEIVER_HEX"
echo "Token contract: $TOKEN_CONTRACT"
echo "Amount: $AMOUNT"
echo "Client timestamp: $CLIENT_TIMESTAMP"

stellar contract invoke \
  --id "$HOT_STELLAR_LOCKER_CONTRACT" \
  --source-account "$STELLAR_SECRET_KEY" \
  --rpc-url "$STELLAR_RPC_URL" \
  --network-passphrase "$STELLAR_NETWORK_PASSPHRASE" \
  --send=yes \
  -- \
  deposit \
  --sender_id "$SENDER_ACCOUNT" \
  --receiver_id "$RECEIVER_HEX" \
  --token "$TOKEN_CONTRACT" \
  --client_timestamp "$CLIENT_TIMESTAMP" \
  --amount "$AMOUNT"
