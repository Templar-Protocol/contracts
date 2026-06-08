#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

HOT_STELLAR_LOCKER_CONTRACT="${HOT_STELLAR_LOCKER_CONTRACT:-CCLWL5NYSV2WJQ3VBU44AMDHEVKEPA45N2QP2LL62O3JVKPGWWAQUVAG}"
STELLAR_RPC_URL="${STELLAR_RPC_URL:-https://mainnet.sorobanrpc.com}"
STELLAR_NETWORK_PASSPHRASE="${STELLAR_NETWORK_PASSPHRASE:-Public Global Stellar Network ; September 2015}"
STELLAR_SOURCE_IDENTITY="${STELLAR_SOURCE_IDENTITY:-templar-hot-mainnet}"
PROVEN_HOT_RECEIVER_HEX="52fd581de41f4bace88c936b89bf267a1161426a466adc518cd9e56f201651dd"
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

if ! SENDER_ACCOUNT="$(stellar keys address "$STELLAR_SOURCE_IDENTITY")"; then
  echo "failed to resolve Stellar sender identity: $STELLAR_SOURCE_IDENTITY" >&2
  exit 1
fi

if [ "$ASSET_KIND" != "native" ]; then
  echo "Only native XLM is supported by this proof script right now" >&2
  exit 1
fi

TOKEN_CONTRACT="$(stellar contract id asset --asset native --rpc-url "$STELLAR_RPC_URL" --network-passphrase "$STELLAR_NETWORK_PASSPHRASE")"
CLIENT_TIMESTAMP="$(python3 - <<'PY'
import time
print(time.time_ns() - 20_000_000_000)
PY
)"
RECEIVER_HEX="${RECEIVER_HEX_OVERRIDE:-${HOT_RECEIVER_HEX:-$PROVEN_HOT_RECEIVER_HEX}}"
if [[ ! "$RECEIVER_HEX" =~ ^[0-9A-Fa-f]{64}$ ]]; then
  echo "receiver hex must be exactly 64 hex characters" >&2
  exit 1
fi

echo "HOT locker contract: $HOT_STELLAR_LOCKER_CONTRACT"
echo "Stellar sender: $SENDER_ACCOUNT"
echo "NEAR receiver: $TARGET_NEAR_ACCOUNT"
echo "Receiver hash: $RECEIVER_HEX"
echo "Token contract: $TOKEN_CONTRACT"
echo "Amount: $AMOUNT"
echo "Client timestamp: $CLIENT_TIMESTAMP"

stellar contract invoke \
  --id "$HOT_STELLAR_LOCKER_CONTRACT" \
  --source-account "$STELLAR_SOURCE_IDENTITY" \
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
