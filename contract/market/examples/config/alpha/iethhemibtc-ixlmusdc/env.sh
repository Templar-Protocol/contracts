#!/usr/bin/env bash
set -Eeuo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

export MARKET_ARGS_FILE="$SCRIPT_DIR/market-args.json"
export COLLATERAL_PRICE_ID="cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
export PROXY_COLLATERAL_ARGS_FILE="$SCRIPT_DIR/proxy-collateral.json"
export BORROW_PRICE_ID="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
export PROXY_BORROW_ARGS_FILE="$SCRIPT_DIR/proxy-borrow.json"

export NETWORK=mainnet
export SIGNER_ID="templar-alpha.near"
export REGISTRY_ID="templar-alpha.near"
export MARKET_NAME="iethhemibtc-ixlmusdc"
export MARKET_VERSION_KEY="templar-market-contract@1.4.0#82543698e6be1dd4cb47bb68da92f78d6f8fac1bf2a2297ba7bdc92ef12d96d7"
export PROXY_ORACLE_VERSION_KEY="templar-proxy-oracle-near-contract@0.2.0#85816cf37cc1661fdda8da629d7a1a74c4786c658a8718c921653c2710616a5c"
export PROXY_GOVERNANCE_VERSION_KEY="templar-proxy-oracle-near-governance-contract@0.1.0#e122c896c930e79da0e3a4d20d18a25558c437728fd9f497338d82f61e95d88a"
