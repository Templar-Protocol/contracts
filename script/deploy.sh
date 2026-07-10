#!/usr/bin/env bash
# Deploy a market with its own proxy oracle and a separate governance contract.
#
# Architecture: the proxy oracle holds owner-gated `admin_*` mutators; the
# governance contract is a distinct account that administers the oracle by
# executing proposals that call into those mutators. So governance must be the
# oracle's owner before it can configure any price feeds. Both account ids are
# derived below, so the oracle is initialized with governance as its owner
# directly and no ownership handoff is needed.
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

# `GOVERNANCE_ID` is interpolated into the oracle's init-args JSON below. NEAR
# account ids cannot contain JSON metacharacters, so pinning the charset here
# is what makes that interpolation safe.
if [[ ! "$GOVERNANCE_ID" =~ ^[a-z0-9._-]+$ ]]; then
    echo "Derived governance id is not a valid NEAR account id: $GOVERNANCE_ID" >&2
    echo "(check MARKET_NAME and REGISTRY_ID in $ENV_FILE)" >&2
    exit 1
fi

# The oracle's `new` gained its `owner_id` argument in 0.3.0. An older wasm's
# `new` takes no arguments and would reject the init args below, leaving the
# freshly created account holding uninitialized code. Version keys are
# `{package_name}@{version}#{sha256}`, so read the version back out and refuse
# to deploy anything older. Only exact `major.minor.patch` releases are
# accepted: `sort -V` orders `0.3.0-rc1` *after* `0.3.0`, the opposite of
# semver, so a pre-release must be rejected outright rather than compared.
PROXY_ORACLE_MIN_VERSION="0.3.0"
proxy_oracle_version="${PROXY_ORACLE_VERSION_KEY##*@}"
proxy_oracle_version="${proxy_oracle_version%%#*}"

if [[ ! "$proxy_oracle_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "PROXY_ORACLE_VERSION_KEY does not name a release version of the proxy oracle." >&2
    echo "Expected {name}@{major.minor.patch}#{sha256}, got: $PROXY_ORACLE_VERSION_KEY" >&2
    exit 1
fi

IFS=. read -r have_major have_minor have_patch <<<"$proxy_oracle_version"
IFS=. read -r want_major want_minor want_patch <<<"$PROXY_ORACLE_MIN_VERSION"

if ((10#$have_major < 10#$want_major ||
    (10#$have_major == 10#$want_major && 10#$have_minor < 10#$want_minor) ||
    (10#$have_major == 10#$want_major && 10#$have_minor == 10#$want_minor && 10#$have_patch < 10#$want_patch))); then
    echo "PROXY_ORACLE_VERSION_KEY names proxy oracle $proxy_oracle_version, but this script requires >= $PROXY_ORACLE_MIN_VERSION" >&2
    echo "(it initializes the oracle with an explicit owner_id, which older versions do not accept)" >&2
    echo "Register the >= $PROXY_ORACLE_MIN_VERSION wasm with 'tmplrmgr registry add-version' and update PROXY_ORACLE_VERSION_KEY in $ENV_FILE" >&2
    exit 1
fi

TMPLRMGR_GLOBAL_ARGS=(
    --network "$NETWORK"
)

# operator-signed call (governance admin / deployer). Credentials are per-write
# args sourced from SIGNER_ID/SECRET_KEY in the environment, so they no longer
# precede the subcommand (they're structural on each write command now).
operator() {
    SIGNER_ID="$SIGNER_ID" SECRET_KEY="$SECRET_KEY" \
        tmplrmgr "${TMPLRMGR_GLOBAL_ARGS[@]}" "$@"
}

# The oracle names governance as its owner before that account exists. This is
# safe: `registry deploy` is one atomic promise batch that drops its reservation
# on failure, and GOVERNANCE_ID is derived, so a failed governance step is fixed
# by re-running it at the same id. Only the oracle step is not re-runnable — a
# second attempt collides on the market id.
echo "Deploying proxy oracle ($PROXY_ORACLE_ID), owned by $GOVERNANCE_ID..."
operator registry deploy \
    --registry-id "$REGISTRY_ID" \
    --name "$PROXY_ORACLE_NAME" \
    --version-key "$PROXY_ORACLE_VERSION_KEY" \
    --init-args "$(printf '{"owner_id":"%s"}' "$GOVERNANCE_ID")" \
    --deposit "5 NEAR"

echo "Deploying governance ($GOVERNANCE_ID)..."
operator proxy-oracle-governance create \
    --registry-id "$REGISTRY_ID" \
    --name "$GOVERNANCE_NAME" \
    --version-key "$PROXY_GOVERNANCE_VERSION_KEY" \
    --proxy-oracle-id "$PROXY_ORACLE_ID" \
    --admin-id "$SIGNER_ID" \
    --ttl-default 0s \
    --deposit "3.5 NEAR"

# --ttl-default 0s (above) makes every proposal executable immediately, so
# --execute-when-ready creates and executes each in a single call.
echo "Configuring collateral proxy..."
operator proxy-oracle-governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
    --execute-when-ready \
    set-proxy \
    --price-id "$COLLATERAL_PRICE_ID" \
    --proxy-file "$PROXY_COLLATERAL_ARGS_FILE"

echo "Configuring borrow proxy..."
operator proxy-oracle-governance create-proposal \
    --governance-id "$GOVERNANCE_ID" \
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
