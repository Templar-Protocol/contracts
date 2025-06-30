#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--network:NETWORK" "$@"

if [ -z "$NETWORK" ]; then
  NETWORK="testnet"
fi

1>&2 echo "Checking whether ${ACCOUNT_ID} exists on ${NETWORK}"

OUTPUT=$(curl --silent --request POST \
  --url "https://archival-rpc.$NETWORK.near.org/" \
  --header 'content-type: application/json' \
  --data "$(jq --null-input --arg account_id "${ACCOUNT_ID}" '{
    jsonrpc: "2.0",
    id: "dontcare",
    method: "query",
    params: {
      request_type: "view_account",
      finality: "final",
      account_id: $account_id
    }
  }')")

RESULT=$(<<<"$OUTPUT" jq '.result.block_height')

if [[ -n "$RESULT" && "$RESULT" -gt 0 ]]; then
    echo 1
fi
