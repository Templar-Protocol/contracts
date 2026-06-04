#!/usr/bin/env bash
# Load STELLAR_SECRET_KEY from the Stellar CLI keystore into the current shell.
#
# Usage:
#   source contract/vault-counterparty/scripts/load-stellar-secret-env.sh [identity]
#
# The secret is exported for child processes but never printed.

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  cat >&2 <<'EOF'
This script must be sourced so it can update your current shell:

  source contract/vault-counterparty/scripts/load-stellar-secret-env.sh [identity]

The identity defaults to STELLAR_KEY_NAME, then templar-hot-mainnet.
EOF
  exit 2
fi

set -euo pipefail

identity="${1:-${STELLAR_KEY_NAME:-templar-hot-mainnet}}"
config_args=()
if [[ -n "${STELLAR_CONFIG_DIR:-}" ]]; then
  config_args=(--config-dir "$STELLAR_CONFIG_DIR")
fi

if ! command -v stellar >/dev/null 2>&1; then
  echo "stellar CLI not found" >&2
  return 1
fi

secret="$(stellar keys secret "$identity" "${config_args[@]}")"
sender="$(stellar keys address "$identity" "${config_args[@]}")"

if [[ -z "$secret" || -z "$sender" ]]; then
  echo "failed to load Stellar identity: $identity" >&2
  return 1
fi

export STELLAR_KEY_NAME="$identity"
export STELLAR_SECRET_KEY="$secret"
export STELLAR_SENDER_ACCOUNT="$sender"

echo "Loaded STELLAR_SECRET_KEY for Stellar identity '$identity' ($sender)" >&2
