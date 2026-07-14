#!/usr/bin/env bash
# Stop the out-of-band `neard` pool started by sandbox-up.sh (kills each host,
# whose Drop kills its child neard).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

host_bin="${ROOT_DIR}/target/debug/sandbox-host"

# Reap orphans: a pool started while another was live overwrote the pid files, so
# the older pool's nodes are untracked and nothing else would ever kill them.
# Matched on this checkout's binary path, so it cannot touch anything else.
reap_orphans() {
  local orphans
  orphans="$(pgrep -f "^${host_bin}" || true)"
  [ -n "$orphans" ] || return 0
  echo "reaping untracked sandbox hosts: $(echo "$orphans" | tr '\n' ' ')"
  # shellcheck disable=SC2086
  kill $orphans 2>/dev/null || true
}

shopt -s nullglob
# Per-node pool pid files, plus the legacy single-node pid file.
pid_files=("${ROOT_DIR}"/target/.sandbox-host.*.pid "${ROOT_DIR}"/target/.sandbox-host.pid)
if [ ${#pid_files[@]} -eq 0 ]; then
  echo "no sandbox host pid files; nothing to stop"
  reap_orphans
  exit 0
fi

for pid_file in "${pid_files[@]}"; do
  [ -f "$pid_file" ] || continue
  pid="$(cat "$pid_file")"
  # Guard against PID reuse: only kill if the PID is still a sandbox-host.
  if ps -p "$pid" -o args= 2>/dev/null | grep -q "sandbox-host"; then
    if kill "$pid" 2>/dev/null; then
      echo "sandbox down (pid ${pid})"
    fi
  else
    echo "pid ${pid} is not a sandbox-host process; refusing to kill"
  fi
  rm -f "$pid_file"
done

reap_orphans
