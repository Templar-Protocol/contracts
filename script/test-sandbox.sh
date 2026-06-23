#!/usr/bin/env bash
# Run the node-backed (`#[ignore]`-gated) test suite against one shared
# out-of-band `neard`. The `sandbox` nextest profile's setup script starts the
# node and exports its RPC url; this wrapper guarantees teardown on exit.
#
# Usage: script/test-sandbox.sh [extra nextest args / filters]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
trap '"${SCRIPT_DIR}/sandbox-down.sh" || true' EXIT

cargo nextest run --profile sandbox --run-ignored all "$@"
