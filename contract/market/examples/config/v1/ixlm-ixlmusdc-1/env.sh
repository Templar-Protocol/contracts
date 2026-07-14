#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

export MARKET_ARGS_FILE="$SCRIPT_DIR/market-args.json"
export COLLATERAL_PRICE_ID="cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
export PROXY_COLLATERAL_ARGS_FILE="$SCRIPT_DIR/proxy-collateral.json"
export BORROW_PRICE_ID="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
export PROXY_BORROW_ARGS_FILE="$SCRIPT_DIR/proxy-borrow.json"

export NETWORK=mainnet
export SIGNER_ID="tmplr.near"
export REGISTRY_ID="v1.tmplr.near"
export MARKET_NAME="ixlm-ixlmusdc-1"
export MARKET_VERSION_KEY="v1.3.0"
export PROXY_ORACLE_VERSION_KEY="templar-proxy-oracle-near-contract@0.3.0#d2e62c4566c98e55121a5aad32e0e5b8cfb911f82aca71dbaeaa83794fed9e8e"
export PROXY_GOVERNANCE_VERSION_KEY="templar-proxy-oracle-near-governance-contract@0.1.0#09ecfafa86bfdca5e05b9174590cd056d59bf3a9d8727e9d452cfb98701334b0"
