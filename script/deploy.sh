#!/usr/bin/env bash
# Usage: SECRET_KEY=... ./deploy.sh ./<market>/env.sh

set -Eeuo pipefail

ENV_FILE=${1:-}
if [[ -z "$ENV_FILE" ]]; then
    echo "Usage: SECRET_KEY=... ./deploy.sh ./<market>/env.sh" >&2
    exit 1
fi

if [[ ! -f "$ENV_FILE" ]]; then
    echo "Config file not found: $ENV_FILE" >&2
    exit 1
fi

# shellcheck source=/dev/null
source "$ENV_FILE"

required_vars=(
    SECRET_KEY
    MARKET_ARGS_FILE
    PROXY_COLLATERAL_ARGS_FILE
    PROXY_BORROW_ARGS_FILE
    NETWORK
    SIGNER_ID
    REGISTRY_ID
    MARKET_NAME
    MARKET_VERSION_KEY
    PROXY_ORACLE_VERSION_KEY
)

for required_var in "${required_vars[@]}"; do
    if [[ -z "${!required_var:-}" ]]; then
        echo "Missing required environment variable: $required_var" >&2
        exit 1
    fi
done

required_files=(
    "$MARKET_ARGS_FILE"
    "$PROXY_COLLATERAL_ARGS_FILE"
    "$PROXY_BORROW_ARGS_FILE"
)

for required_file in "${required_files[@]}"; do
    if [[ ! -f "$required_file" ]]; then
        echo "Required file not found: $required_file" >&2
        exit 1
    fi
done

# derived values
MARKET_ID="$MARKET_NAME.$REGISTRY_ID"
PROXY_ORACLE_NAME="proxy-oracle-$MARKET_NAME"
PROXY_ORACLE_ID="$PROXY_ORACLE_NAME.$REGISTRY_ID"

# script
echo "Deploying proxy oracle..."
tmplrmgr proxy-oracle deploy from-registry \
    --registry-id "$REGISTRY_ID" \
    --version-key "$PROXY_ORACLE_VERSION_KEY" \
    --name "$PROXY_ORACLE_NAME" \
    --deposit "3.5 NEAR"

echo "Proposing proxy oracle owner..."
near contract call-function as-transaction "$PROXY_ORACLE_ID" own_propose_owner \
    json-args "{\"account_id\":\"$SIGNER_ID\"}" \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as "$REGISTRY_ID" \
    network-config "$NETWORK" \
    sign-with-plaintext-private-key "$SECRET_KEY" \
    send

echo "Accepting proxy oracle owner..."
near contract call-function as-transaction "$PROXY_ORACLE_ID" own_accept_owner \
    json-args "{}" \
    prepaid-gas '100.0 Tgas' \
    attached-deposit '1 yoctoNEAR' \
    sign-as "$SIGNER_ID" \
    network-config "$NETWORK" \
    sign-with-plaintext-private-key "$SECRET_KEY" \
    send

echo "Creating collateral proxy..."
tmplrmgr proxy-oracle governance create \
    --oracle-id "$PROXY_ORACLE_ID" \
    --id 0 \
    proxy \
    --price-id "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" \
    --insert-file "$PROXY_COLLATERAL_ARGS_FILE"

tmplrmgr proxy-oracle governance execute \
    --oracle-id "$PROXY_ORACLE_ID" \
    --id 0

echo "Creating borrow proxy..."
tmplrmgr proxy-oracle governance create \
    --oracle-id "$PROXY_ORACLE_ID" \
    --id 1 \
    proxy \
    --price-id "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" \
    --insert-file "$PROXY_BORROW_ARGS_FILE"

tmplrmgr proxy-oracle governance execute \
    --oracle-id "$PROXY_ORACLE_ID" \
    --id 1

echo "Deploying market..."
tmplrmgr market deploy from-registry \
    --registry-id "$REGISTRY_ID" \
    --version-key "$MARKET_VERSION_KEY" \
    --name "$MARKET_NAME" \
    --args-file "$MARKET_ARGS_FILE" \
    --deposit "5.5 NEAR"
