#!/usr/bin/env bash
# Start a pool of out-of-band `neard` instances for attach-mode tests and export:
#   NEAR_SANDBOX_RPC_URL_<i>  - the i-th node's RPC url (i in 0..count-1)
#   NEAR_SANDBOX_RPC_URL      - node 0 (fallback for manual runs)
#   TEST_CONTRACTS_PREBUILT    - confirms contract Wasms were built
#
# Source this script so the exports remain available to the test process.
#
# Each test attaches to the node for its NEXTEST_TEST_GLOBAL_SLOT, so a node is
# used by at most one test at a time (exclusive: fast_forward and chain state
# stay isolated between concurrent tests) yet reused across the tests that pass
# through that slot (no per-test boot/teardown). Test orchestration must set
# SANDBOX_NODE_COUNT from the same value passed to nextest's --test-threads.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
NODE_COUNT="${SANDBOX_NODE_COUNT:?SANDBOX_NODE_COUNT must be set by test orchestration}"

# The pool's addr/pid files are fixed paths, so starting a pool on top of a live
# one overwrites them and orphans the older nodes — nothing tracks them after
# that, and they linger holding RAM. Starting a pool means replacing whatever is
# already there.
bash "${SCRIPT_DIR}/sandbox-down.sh" >/dev/null

# Prebuild the contract wasms once so tests don't each recompile them.
bash "${SCRIPT_DIR}/prebuild-test-contracts.sh"

cargo build -q -p templar-gateway-testing --bin sandbox-host

addr_file() { echo "${ROOT_DIR}/target/.sandbox-host-url.${1}"; }
pid_file() { echo "${ROOT_DIR}/target/.sandbox-host.${1}.pid"; }
log_file() { echo "${ROOT_DIR}/target/.sandbox-host.${1}.log"; }

start_node() {
  local i="$1"
  rm -f "$(addr_file "$i")"
  nohup "${ROOT_DIR}/target/debug/sandbox-host" "$(addr_file "$i")" \
    >"$(log_file "$i")" 2>&1 &
  echo $! >"$(pid_file "$i")"
}

wait_for_node() {
  local i="$1"
  for _ in $(seq 1 180); do
    [ -s "$(addr_file "$i")" ] && return 0
    sleep 1
  done
  echo "sandbox node ${i} did not report an RPC url; see $(log_file "$i")" >&2
  bash "${SCRIPT_DIR}/sandbox-down.sh" || true
  exit 1
}

# Start node 0 first and wait, so the neard binary is fetched/cached before the
# remaining nodes start concurrently (avoids a first-run download race).
start_node 0
wait_for_node 0
for i in $(seq 1 $((NODE_COUNT - 1))); do
  start_node "$i"
done
for i in $(seq 1 $((NODE_COUNT - 1))); do
  wait_for_node "$i"
done

for i in $(seq 0 $((NODE_COUNT - 1))); do
  _sandbox_rpc_url="$(cat "$(addr_file "$i")")"
  _sandbox_pid="$(cat "$(pid_file "$i")")"
  echo "sandbox node ${i} up at ${_sandbox_rpc_url} (pid ${_sandbox_pid})"
  export "NEAR_SANDBOX_RPC_URL_${i}=${_sandbox_rpc_url}"
  if [ "$i" -eq 0 ]; then
    export NEAR_SANDBOX_RPC_URL="${_sandbox_rpc_url}"
  fi
done
export TEST_CONTRACTS_PREBUILT=1
unset _sandbox_rpc_url _sandbox_pid
