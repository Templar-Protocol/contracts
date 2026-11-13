#!/usr/bin/env bash
set -euo pipefail

# The fixed Python runner owns checkpoint selection and argument filtering.
# Every numbered stage still executes as a real process and stops on nonzero.
exec python3 "$(dirname -- "${BASH_SOURCE[0]}")/common.py" run "$@"
