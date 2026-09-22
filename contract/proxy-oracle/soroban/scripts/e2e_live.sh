#!/usr/bin/env bash
# Thin entrypoint for the restart-safe testnet rehearsal.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
exec python3 "$ROOT/contract/proxy-oracle/soroban/scripts/e2e_state.py" "$@"
