#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

export MARKET_ARGS_FILE="$SCRIPT_DIR/market-args.json"
export PROXY_COLLATERAL_ARGS_FILE="$SCRIPT_DIR/proxy-collateral.json"
export PROXY_BORROW_ARGS_FILE="$SCRIPT_DIR/proxy-borrow.json"

export NETWORK=mainnet
export SIGNER_ID="tmplr.near"
export REGISTRY_ID="v1.tmplr.near"
export MARKET_NAME="stnear-usdt"
export MARKET_VERSION_KEY="v1.3.0"
export PROXY_ORACLE_VERSION_KEY="templar-proxy-oracle-contract@0.1.0#e877687e2d6f51db824bde12348938b3374f526301811df3ee118af38b856f35"
