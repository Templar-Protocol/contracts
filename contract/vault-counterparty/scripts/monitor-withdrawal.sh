#!/usr/bin/env bash

set -euo pipefail

nonce="${1:?usage: monitor-withdrawal.sh <nonce> [receiver] [interval]}"
receiver="${2:-}"
interval="${3:-15}"

if [ -z "$receiver" ]; then
  receiver="${STELLAR_RECEIVER:?set STELLAR_RECEIVER or pass receiver}"
fi

while true; do
  echo "===== $(date -u '+%Y-%m-%dT%H:%M:%SZ') ====="
  echo "nonce: $nonce"
  echo "receiver: $receiver"

  echo "-- stellar native balance --"
  python3 -c 'import json, sys, urllib.request; addr = sys.argv[1]; data = json.load(urllib.request.urlopen(f"https://horizon.stellar.org/accounts/{addr}", timeout=30)); [print(balance.get("balance")) for balance in data.get("balances", []) if balance.get("asset_type") == "native"]' "$receiver" || true

  echo "-- hot withdraw/sign --"
  node -e 'const nonce = process.argv[1]; for (const url of ["https://rpc1.hotdao.ai/withdraw/sign", "https://rpc2.hotdao.ai/withdraw/sign"]) { try { const res = await fetch(url, {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify({nonce})}); console.log(url, res.status, await res.text()); } catch (error) { console.log(url, "ERR", String(error)); } }' "$nonce"

  echo "-- hot clear_completed_withdrawal --"
  node -e 'const nonce = process.argv[1]; const headers = {"omni-version": "v2", "Content-Type": "application/json", "Referer": "https://near-intents.org", "Origin": "https://near-intents.org"}; for (const url of ["https://api0.herewallet.app/api/v1/transactions/clear_completed_withdrawal", "https://api2.herewallet.app/api/v1/transactions/clear_completed_withdrawal"]) { try { const res = await fetch(url, {method: "POST", headers, body: JSON.stringify({nonce})}); console.log(url, res.status, await res.text()); } catch (error) { console.log(url, "ERR", String(error)); } }' "$nonce"

  if [ -n "${HOT_STELLAR_ENCODED_RECEIVER:-}" ]; then
    echo "-- near pending withdrawals --"
    python3 -c 'import base64, json, sys, urllib.request; encoded, account_id, rpc_url = sys.argv[1:]; args = {"receiver_id": encoded, "chain_id": 1100}; payload = {"jsonrpc": "2.0", "id": "dontcare", "method": "query", "params": {"request_type": "call_function", "finality": "final", "account_id": account_id, "method_name": "get_withdrawals_by_receiver", "args_base64": base64.b64encode(json.dumps(args).encode()).decode()}}; request = urllib.request.Request(rpc_url, data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"}); print(urllib.request.urlopen(request, timeout=60).read().decode())' "$HOT_STELLAR_ENCODED_RECEIVER" "${OMNI_CONTRACT:?set OMNI_CONTRACT}" "${HOT_OMNI_NEAR_RPC_URL:?set HOT_OMNI_NEAR_RPC_URL}" || true
  else
    echo "-- near pending withdrawals skipped; set HOT_STELLAR_ENCODED_RECEIVER to query by HOT receiver id --"
  fi

  if [ "${ONCE:-0}" = "1" ]; then
    break
  fi
  sleep "$interval"
done
