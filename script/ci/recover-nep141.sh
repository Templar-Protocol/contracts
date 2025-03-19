#!/usr/bin/env bash
set -e

while [[ $# -gt 0 ]]; do
  case "$1" in
    -a|--account)
      ACCOUNT_ID="$2"
      shift 2
      ;;
    -t|--token)
      TOKEN_ID="$2"
      shift 2
      ;;
    -n|--network)
      NETWORK="$2"
      shift 2
      ;;
    -b|--beneficiary)
      BENEFICIARY_ID="$2"
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

echo "Recovering $TOKEN_ID tokens for $ACCOUNT_ID on $NETWORK"

echo "Transferring balance to $BENEFICIARY_ID"

( set +e; # send all errors if balance is zero
near tokens "$ACCOUNT_ID" send-ft "$TOKEN_ID" "$BENEFICIARY_ID" all memo "" \
  network-config "$NETWORK" \
  sign-with-plaintext-private-key \
    --signer-public-key "$PUBLIC_KEY" \
    --signer-private-key "$PRIVATE_KEY" \
  send)

echo "Performing storage unregistration"

near contract call-function as-transaction "$TOKEN_ID" storage_unregister \
  json-args '{"force":true}' \
  prepaid-gas '100.0 Tgas' \
  attached-deposit '0 NEAR' \
  sign-as "$ACCOUNT_ID" \
  network-config "$NETWORK" \
  sign-with-plaintext-private-key \
    --signer-public-key "$PUBLIC_KEY" \
    --signer-private-key "$PRIVATE_KEY" \
  send

echo "Done"
