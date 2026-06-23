#!/usr/bin/env bash
# Stop the out-of-band `neard` started by sandbox-up.sh (kills the host, whose
# Drop kills the child neard).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_FILE="${ROOT_DIR}/target/.sandbox-host.pid"

if [ -f "$PID_FILE" ]; then
  PID="$(cat "$PID_FILE")"
  if kill "$PID" 2>/dev/null; then
    echo "sandbox down (pid ${PID})"
  fi
  rm -f "$PID_FILE"
else
  echo "no sandbox host pid file; nothing to stop"
fi
