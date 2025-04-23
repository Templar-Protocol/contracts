#!/usr/bin/env bash
set -e

SCRIPT_DIR=$(dirname "$(readlink -f ${BASH_SOURCE[0]})")
source "$SCRIPT_DIR/utils.sh"

parse_args "--account:ACCOUNT_ID,--beneficiary:BENEFICIARY_ID,--network:NETWORK,--private-key:PRIVATE_KEY,--public-key:PUBLIC_KEY" "$@"

if [ -z "$NETWORK" ]; then
    NETWORK="testnet"
fi

VERSIONS=$(near --quiet contract call-function as-read-only "${ACCOUNT_ID}" list_versions \
    json-args "{}" \
    network-config "${NETWORK}" \
    now)

echo "Versions"
echo "$VERSIONS" | jq .

echo "${VERSIONS}" | jq -r '.[]' | while read VERSION_KEY; do
    echo "Removing ${VERSION_KEY}..."

    near contract call-function as-transaction "${ACCOUNT_ID}" remove_version \
        json-args "{\"version_key\":\"${VERSION_KEY}\"}" \
        prepaid-gas '200.0 Tgas' \
        attached-deposit '1 yoctoNEAR' \
        sign-as "${ACCOUNT_ID}" \
        network-config testnet \
        sign-with-plaintext-private-key \
          --signer-public-key "${PUBLIC_KEY}" \
          --signer-private-key "${PRIVATE_KEY}" \
        send
done

near account delete-account "${ACCOUNT_ID}" \
  beneficiary "${BENEFICIARY_ID}" \
  network-config "${NETWORK}" \
  sign-with-plaintext-private-key \
    --signer-public-key "${PUBLIC_KEY}" \
    --signer-private-key "${PRIVATE_KEY}" \
  send

echo "Done"
