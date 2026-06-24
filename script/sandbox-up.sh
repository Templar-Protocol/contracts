#!/usr/bin/env bash
# Start a pool of out-of-band `neard` instances for attach-mode tests.
#
# Boots SANDBOX_NODE_COUNT nodes (default 4). When run as a nextest setup script
# for the `sandbox` profile it exports, into the test environment:
#   NEAR_SANDBOX_RPC_URL_<i>  - the i-th node's RPC url (i in 0..count-1)
#   NEAR_SANDBOX_RPC_URL      - node 0 (fallback for non-nextest/manual runs)
#   SANDBOX_NODE_COUNT, TEST_CONTRACTS_PREBUILT
#
# Each test attaches to the node for its NEXTEST_TEST_GLOBAL_SLOT, so a node is
# used by at most one test at a time (exclusive: fast_forward and chain state
# stay isolated between concurrent tests) yet reused across the tests that pass
# through that slot (no per-test boot/teardown). The node count MUST be >= the
# sandbox profile's `test-threads` in .config/nextest.toml; keep them in sync
# (override both via SANDBOX_NODE_COUNT and NEXTEST_TEST_THREADS to retune).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
NODE_COUNT="${SANDBOX_NODE_COUNT:-4}"

# Prebuild the contract wasms once so tests don't each recompile them. Skip with
# SANDBOX_SKIP_PREBUILD=1 when they are known to be current.
if [ -z "${SANDBOX_SKIP_PREBUILD:-}" ]; then
  bash "${SCRIPT_DIR}/prebuild-test-contracts.sh"
fi

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
  echo "sandbox node ${i} up at $(cat "$(addr_file "$i")") (pid $(cat "$(pid_file "$i")"))"
done

# Export to the nextest test environment when invoked as a setup script.
if [ -n "${NEXTEST_ENV:-}" ]; then
  {
    echo "NEAR_SANDBOX_RPC_URL=$(cat "$(addr_file 0)")"
    echo "SANDBOX_NODE_COUNT=${NODE_COUNT}"
    for i in $(seq 0 $((NODE_COUNT - 1))); do
      echo "NEAR_SANDBOX_RPC_URL_${i}=$(cat "$(addr_file "$i")")"
    done
    echo "TEST_CONTRACTS_PREBUILT=1"
  } >>"$NEXTEST_ENV"
fi
