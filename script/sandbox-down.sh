#!/usr/bin/env bash
# Stop the out-of-band `neard` started by sandbox-up.sh (kills the host, whose
# Drop kills the child neard).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_FILE="${ROOT_DIR}/target/.sandbox-host.pid"

if [ -f "$PID_FILE" ]; then
  PID="$(cat "$PID_FILE")"
  # Guard against PID reuse: only kill if the PID is still a sandbox-host.
  if ps -p "$PID" -o args= 2>/dev/null | grep -q "sandbox-host"; then
    if kill "$PID" 2>/dev/null; then
      echo "sandbox down (pid ${PID})"
    fi
  else
    echo "pid ${PID} is not a sandbox-host process; refusing to kill"
  fi
  rm -f "$PID_FILE"
else
  echo "no sandbox host pid file; nothing to stop"
fi
