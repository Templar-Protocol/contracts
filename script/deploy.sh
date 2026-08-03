#!/usr/bin/env bash
# Deploy a market with its own proxy oracle and a separate governance contract.
#
# Governance administers the oracle's owner-gated `admin_*` mutators by executing
# proposals against them, so it must own the oracle before any feed can be
# configured. Both account ids are derived below, so governance deploys first and
# the oracle names it as owner at init — no ownership handoff.
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

# derived values
PROXY_ORACLE_NAME="proxy-oracle-$MARKET_NAME"
PROXY_ORACLE_ID="$PROXY_ORACLE_NAME.$REGISTRY_ID"
GOVERNANCE_NAME="proxy-gov-$MARKET_NAME"
GOVERNANCE_ID="$GOVERNANCE_NAME.$REGISTRY_ID"

TMPLRMGR_GLOBAL_ARGS=(
    --network "$NETWORK"
)

# operator-signed call (governance admin / deployer)
operator() {
    SIGNER_ID="$SIGNER_ID" SECRET_KEY="$SECRET_KEY" \
        tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" "$@"
}

# Ordering is the safety property: `registry deploy` fails the whole transaction if
# the account already exists, so a colliding governance id aborts here, before the
# oracle below could be handed to an account this script did not create. It also
# means the script is not re-runnable end to end — a second run collides.
#
# The cost of that ordering: PROXY_ORACLE_VERSION_KEY is not validated until the
# oracle step, which is *after* this one. A key that is stale (< 0.3.0, whose `new`
# would ignore --owner-id) or malformed aborts there, leaving this governance
# contract deployed and orphaned — delete $GOVERNANCE_ID before re-running.
# Validating up front needs a preflight this CLI has no flag for; ENG-463 removes
# the version check altogether by reading the contract's ABI.
# --ttl-default 0s builds a uniform, override-free policy: every target method is
# Admin-only with no timelock, which is what lets the configuration proposals below
# execute immediately. That is a bring-up policy, not a production one -- harden to
# contract/proxy-oracle/governance-policy.example.json afterwards (see the README's
# "Bring-up and hardening" section for the required ordering).
echo "Deploying governance ($GOVERNANCE_ID)..."
operator proxy-oracle governance create \
    --registry-id "$REGISTRY_ID" \
    --name "$GOVERNANCE_NAME" \
    --version-key "$PROXY_GOVERNANCE_VERSION_KEY" \
    --proxy-oracle-id "$PROXY_ORACLE_ID" \
    --admin-id "$SIGNER_ID" \
    --ttl-default 0s \
    --deposit "3.5 NEAR"

echo "Deploying proxy oracle ($PROXY_ORACLE_ID), owned by $GOVERNANCE_ID..."
operator proxy-oracle create \
    --registry-id "$REGISTRY_ID" \
    --name "$PROXY_ORACLE_NAME" \
    --version-key "$PROXY_ORACLE_VERSION_KEY" \
    --owner-id "$GOVERNANCE_ID" \
    --deposit "5 NEAR"

# --ttl-default 0s (above) makes every proposal executable immediately, so
# --execute-when-ready creates and executes each in a single call.
echo "Configuring collateral proxy..."
operator proxy-oracle governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
    --execute-when-ready \
    oracle set-proxy \
    --price-id "$COLLATERAL_PRICE_ID" \
    --proxy-file "$PROXY_COLLATERAL_ARGS_FILE"

echo "Configuring borrow proxy..."
operator proxy-oracle governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
    --execute-when-ready \
    oracle set-proxy \
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
