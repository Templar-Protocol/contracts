#!/usr/bin/env bash
set -e

while [[ $# -gt 0 ]]; do
    case "$1" in
        -a|--account)
            ACCOUNT_ID="$2"
            shift 2
            ;;
        -c|--contract)
            CONTRACT_ID="$2"
            shift 2
            ;;
        -n|--network)
            NETWORK="$2"
            shift 2
            ;;
        -s|--private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        -v|--public-key)
            PUBLIC_KEY="$2"
            shift 2
            ;;
        *)
            echo "Invalid option: $1"
            exit 1
            ;;
    esac
done

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

near account \
  manage-storage-deposit "${CONTRACT_ID}" \
  deposit "${ACCOUNT_ID}" '0.00125 NEAR' \
  sign-as "${ACCOUNT_ID}" \
  network-config "${NETWORK}" \
  sign-with-plaintext-private-key \
    --signer-public-key "${PUBLIC_KEY}" \
    --signer-private-key "${PRIVATE_KEY}" \
  send

echo "Done"
