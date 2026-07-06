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
PROXY_ORACLE_NAME="proxy-oracle-$MARKET_NAME"
PROXY_ORACLE_ID="$PROXY_ORACLE_NAME.$REGISTRY_ID"

TMPLRMGR_GLOBAL_ARGS=(
    --network "$NETWORK"
)

# script
echo "Deploying proxy oracle..."
tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    registry deploy \
    --registry-id "$REGISTRY_ID" \
    --name "$PROXY_ORACLE_NAME" \
    --version-key "$PROXY_ORACLE_VERSION_KEY" \
    --deposit "3.5 NEAR"

echo "Proposing proxy oracle owner..."
tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$REGISTRY_ID" \
    proxy-oracle-owner propose-owner \
    --oracle-id "$PROXY_ORACLE_ID" \
    --account-id "$SIGNER_ID"

echo "Accepting proxy oracle owner..."
tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    proxy-oracle-owner accept-owner \
    --oracle-id "$PROXY_ORACLE_ID"

echo "Creating collateral proxy..."
tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    proxy-oracle-governance create-proposal \
    --governance-id "$PROXY_ORACLE_ID" \
    --id 0 \
    --operation set-proxy \
    --price-id "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" \
    --proxy-file "$PROXY_COLLATERAL_ARGS_FILE"

tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    proxy-oracle-governance execute-proposal \
    --governance-id "$PROXY_ORACLE_ID" \
    --id 0

echo "Creating borrow proxy..."
tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    proxy-oracle-governance create-proposal \
    --governance-id "$PROXY_ORACLE_ID" \
    --id 1 \
    --operation set-proxy \
    --price-id "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" \
    --proxy-file "$PROXY_BORROW_ARGS_FILE"

tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    proxy-oracle-governance execute-proposal \
    --governance-id "$PROXY_ORACLE_ID" \
    --id 1

echo "Deploying market..."
tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" \
    --signer-id "$SIGNER_ID" \
    market create \
    --registry-id "$REGISTRY_ID" \
    --name "$MARKET_NAME" \
    --version-key "$MARKET_VERSION_KEY" \
    --init-args-file "$MARKET_ARGS_FILE" \
    --deposit "5.5 NEAR"
