#!/usr/bin/env bash
# Deploy a market with its own proxy oracle and a separate governance contract.
#
# Architecture: the proxy oracle holds owner-gated `admin_*` mutators; the
# governance contract is a distinct account that administers the oracle by
# executing proposals that call into those mutators. So governance must become
# the oracle's owner before it can configure any price feeds.
#
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
    COLLATERAL_PRICE_ID
    BORROW_PRICE_ID
    NETWORK
    SIGNER_ID
    REGISTRY_ID
    MARKET_NAME
    MARKET_VERSION_KEY
    PROXY_ORACLE_VERSION_KEY
    PROXY_GOVERNANCE_VERSION_KEY
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

# The registry account owns the oracle immediately after `registry deploy` (it
# is the predecessor of the contract's `new()`), so it signs the owner handoff.
# Every other step is signed by the operator, who is the governance admin. When
# these are different accounts, set REGISTRY_SECRET_KEY; it defaults to SECRET_KEY.
REGISTRY_SECRET_KEY=${REGISTRY_SECRET_KEY:-$SECRET_KEY}

# derived values
PROXY_ORACLE_NAME="proxy-oracle-$MARKET_NAME"
PROXY_ORACLE_ID="$PROXY_ORACLE_NAME.$REGISTRY_ID"
GOVERNANCE_NAME="proxy-governance-$MARKET_NAME"
GOVERNANCE_ID="$GOVERNANCE_NAME.$REGISTRY_ID"

TMPLRMGR_GLOBAL_ARGS=(
    --network "$NETWORK"
)

# operator-signed call (governance admin / deployer)
operator() {
    tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" --signer-id "$SIGNER_ID" --secret-key "$SECRET_KEY" "$@"
}

# registry-signed call (initial oracle owner)
registry() {
    tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" --signer-id "$REGISTRY_ID" --secret-key "$REGISTRY_SECRET_KEY" "$@"
}

echo "Deploying proxy oracle ($PROXY_ORACLE_ID)..."
operator registry deploy \
    --registry-id "$REGISTRY_ID" \
    --name "$PROXY_ORACLE_NAME" \
    --version-key "$PROXY_ORACLE_VERSION_KEY" \
    --deposit "3.5 NEAR"

echo "Deploying governance ($GOVERNANCE_ID)..."
operator proxy-oracle-governance create \
    --registry-id "$REGISTRY_ID" \
    --name "$GOVERNANCE_NAME" \
    --version-key "$PROXY_GOVERNANCE_VERSION_KEY" \
    --proxy-oracle-id "$PROXY_ORACLE_ID" \
    --admin-id "$SIGNER_ID" \
    --ttl-default 0s \
    --deposit "3.5 NEAR"

echo "Proposing governance as the oracle owner..."
registry proxy-oracle-owner propose-owner \
    --oracle-id "$PROXY_ORACLE_ID" \
    --account-id "$GOVERNANCE_ID"

# --ttl-default 0s (above) makes every proposal executable immediately, so
# --execute-when-ready creates and executes each in a single call.
echo "Accepting oracle ownership through governance..."
operator proxy-oracle-governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
    --id 0 \
    --execute-when-ready \
    admin-function-call \
    --method own_accept_owner \
    --deposit "1 yoctoNEAR"

echo "Configuring collateral proxy..."
operator proxy-oracle-governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
    --id 1 \
    --execute-when-ready \
    set-proxy \
    --price-id "$COLLATERAL_PRICE_ID" \
    --proxy-file "$PROXY_COLLATERAL_ARGS_FILE"

echo "Configuring borrow proxy..."
operator proxy-oracle-governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
    --id 2 \
    --execute-when-ready \
    set-proxy \
    --price-id "$BORROW_PRICE_ID" \
    --proxy-file "$PROXY_BORROW_ARGS_FILE"

echo "Deploying market..."
operator market create \
    --registry-id "$REGISTRY_ID" \
    --name "$MARKET_NAME" \
    --version-key "$MARKET_VERSION_KEY" \
    --init-args-file "$MARKET_ARGS_FILE" \
    --deposit "5.5 NEAR"

echo "Done. proxy-oracle=$PROXY_ORACLE_ID governance=$GOVERNANCE_ID market=$MARKET_NAME.$REGISTRY_ID"
