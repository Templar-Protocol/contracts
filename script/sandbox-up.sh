#!/usr/bin/env bash
# Start one out-of-band `neard` for attach-mode tests.
#
# Run standalone to print the RPC url, or as a nextest setup script (for the
# `sandbox` profile) — in which case it exports NEAR_SANDBOX_RPC_URL and
# TEST_CONTRACTS_PREBUILT into the test environment so every test process
# attaches to this one node instead of booting its own.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ADDR_FILE="${ROOT_DIR}/target/.sandbox-host-url"
PID_FILE="${ROOT_DIR}/target/.sandbox-host.pid"
LOG_FILE="${ROOT_DIR}/target/.sandbox-host.log"

# Prebuild the contract wasms once so tests don't each recompile them. Skip with
# SANDBOX_SKIP_PREBUILD=1 when they are known to be current.
if [ -z "${SANDBOX_SKIP_PREBUILD:-}" ]; then
  bash "${SCRIPT_DIR}/prebuild-test-contracts.sh"
fi

# Launch the out-of-band node (detached) and wait for it to report its url.
rm -f "$ADDR_FILE"
cargo build -q -p templar-gateway-testing --bin sandbox-host
nohup "${ROOT_DIR}/target/debug/sandbox-host" "$ADDR_FILE" >"$LOG_FILE" 2>&1 &
echo $! >"$PID_FILE"

for _ in $(seq 1 180); do
  [ -s "$ADDR_FILE" ] && break
  sleep 1
done
if [ ! -s "$ADDR_FILE" ]; then
  echo "sandbox host did not report an RPC url; see $LOG_FILE" >&2
  # Don't leak the half-started host or leave stale metadata behind.
  kill "$(cat "$PID_FILE")" 2>/dev/null || true
  rm -f "$PID_FILE" "$ADDR_FILE"
  exit 1
fi
URL="$(cat "$ADDR_FILE")"
echo "sandbox up at ${URL} (pid $(cat "$PID_FILE"))"

# Export to the nextest test environment when invoked as a setup script.
if [ -n "${NEXTEST_ENV:-}" ]; then
  {
    echo "NEAR_SANDBOX_RPC_URL=${URL}"
    echo "TEST_CONTRACTS_PREBUILT=1"
  } >>"$NEXTEST_ENV"
fi
