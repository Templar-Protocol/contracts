#!/usr/bin/env bash
set -Eeuo pipefail

if [[ $# -ne 1 ]]; then
  printf 'usage: %s IMAGE\n' "$0" >&2
  exit 2
fi

image=$1
fixture_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/fixtures" && pwd)
fixture=${fixture_dir}/stellar

command -v docker >/dev/null 2>&1 || {
  printf 'docker is required\n' >&2
  exit 2
}
[[ -x "$fixture" ]] || {
  printf 'fake Stellar fixture is not executable: %s\n' "$fixture" >&2
  exit 2
}

# The pinned release is parsed from the CLI's single pin table
# (src/release_pin.rs); this harness never hardcodes tag, length, or digest,
# and the CLI never gets a catalog override from this harness.
pin_table=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/src/release_pin.rs
readonly pin_table
release_tag=$(sed -n 's/^pub const RELEASE_TAG: &str = "\([^"]*\)";$/\1/p' "$pin_table")
readonly release_tag
[[ -n $release_tag ]] || {
  printf 'could not parse release tag from %s\n' "$pin_table" >&2
  exit 2
}
runtime_asset=$(sed -n -E 's/^\s*\("([^"]+)", ([0-9_]+), "([^"]+)"\),$/\1/p' "$pin_table" | head -n 1)
runtime_bytes=$(sed -n -E 's/^\s*\("[^"]+", ([0-9_]+), "[^"]+"\),$/\1/p' "$pin_table" | head -n 1 | sed -E 's/_//g')
runtime_sha256=$(sed -n -E 's/^\s*\("[^"]+", [0-9_]+, "([^"]+)"\),$/\1/p' "$pin_table" | head -n 1)
readonly runtime_asset runtime_bytes runtime_sha256
[[ -n $runtime_asset && -n $runtime_bytes && -n $runtime_sha256 ]] || {
  printf 'could not parse runtime asset pin from %s\n' "$pin_table" >&2
  exit 2
}
readonly RELEASE_URL="https://github.com/Templar-Protocol/contracts/releases/download/${release_tag}/${runtime_asset}"
readonly CACHE_ROOT='/home/templar/.cache/templar/soroban-vault-cli/artifacts'
readonly CACHE_FILE="${CACHE_ROOT}/${release_tag}/${runtime_asset}"
readonly EXPECTED_BYTES=$runtime_bytes
readonly EXPECTED_SHA256=$runtime_sha256

suffix="$$-${RANDOM}"
cache_volume="templar-soroban-smoke-cache-${suffix}"
target_volume="templar-soroban-smoke-target-${suffix}"
state_volume="templar-soroban-smoke-state-${suffix}"
stellar_volume="templar-soroban-smoke-stellar-${suffix}"
volumes=("$cache_volume" "$target_volume" "$state_volume" "$stellar_volume")

cleanup() {
  docker volume rm "${volumes[@]}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for volume in "${volumes[@]}"; do
  docker volume create "$volume" >/dev/null
done

docker run --rm --network=host "$image" --version >/dev/null

docker run --rm \
  --network=host \
  --entrypoint /bin/sh \
  --mount "type=bind,src=${fixture},dst=/smoke-fixture/stellar,readonly" \
  --mount "type=volume,src=${cache_volume},dst=${CACHE_ROOT}" \
  --mount "type=volume,src=${target_volume},dst=/workspace/target" \
  --mount "type=volume,src=${state_volume},dst=/workspace/contract/vault/soroban/.deploy-state" \
  --mount "type=volume,src=${stellar_volume},dst=/home/templar/.config/stellar" \
  -e "RELEASE_URL=${RELEASE_URL}" \
  -e "CACHE_ROOT=${CACHE_ROOT}" \
  -e "CACHE_FILE=${CACHE_FILE}" \
  -e "EXPECTED_BYTES=${EXPECTED_BYTES}" \
  -e "EXPECTED_SHA256=${EXPECTED_SHA256}" \
  -e "TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE=${CACHE_ROOT}" \
  -e 'TEMPLAR_SOROBAN_VAULT_LOG=debug' \
  -e 'FAKE_STELLAR_STATE=/home/templar/.config/stellar/smoke-state' \
  -e 'FAKE_STELLAR_LOG=/home/templar/.config/stellar/smoke-state/calls.log' \
  "$image" -ceu '
    export PATH=/smoke-fixture:$PATH
    bin=/usr/local/bin/tmplr-soroban-vault
    cache_file="$CACHE_FILE"
    fake_state="$FAKE_STELLAR_STATE"
    fake_log="$FAKE_STELLAR_LOG"
    mkdir -p "$fake_state"
    : > "$fake_log"

    count() {
      local name=$1
      if [ -f "$fake_state/$name" ]; then
        wc -l < "$fake_state/$name" | tr -d " "
      else
        printf "0\n"
      fi
    }

    printf "fixed release asset: %s\n" "$RELEASE_URL" >&2
    test ! -e "$cache_file"
    if ! "$bin" deploy wasm vault > /tmp/release-first.log 2>&1; then
      cat /tmp/release-first.log >&2
      exit 1
    fi
    test -f "$cache_file"
    test "$(stat -c "%s" "$cache_file")" -eq "$EXPECTED_BYTES"
    test "$(sha256sum "$cache_file" | cut -d " " -f1)" = "$EXPECTED_SHA256"
    test "$(count upload-actual)" -eq 1
    test "$(count build)" -eq 0
    cache_mtime=$(stat -c "%Y" "$cache_file")

    if ! "$bin" deploy wasm vault > /tmp/release-second.log 2>&1; then
      cat /tmp/release-second.log >&2
      exit 1
    fi
    test "$(stat -c "%s" "$cache_file")" -eq "$EXPECTED_BYTES"
    test "$(sha256sum "$cache_file" | cut -d " " -f1)" = "$EXPECTED_SHA256"
    test "$(stat -c "%Y" "$cache_file")" = "$cache_mtime"
    test "$(count upload-actual)" -eq 1
    test "$(count build)" -eq 0
    if [ -s /tmp/release-second.log ] && grep -Eiq "download|https://github.com/Templar-Protocol/contracts/releases/download" /tmp/release-second.log; then
      printf "cache hit emitted unexpected download evidence:\n" >&2
      cat /tmp/release-second.log >&2
      exit 1
    fi

    cache_hash_before_build=$(sha256sum "$cache_file")
    if ! "$bin" deploy wasm vault --build > /tmp/explicit-build.log 2>&1; then
      cat /tmp/explicit-build.log >&2
      exit 1
    fi
    test "$(count build)" -eq 1
    # The fake `stellar` fixture short-circuits the real cargo compile, so
    # probe the pinned soroban toolchain directly: stellar-cli 26 builds
    # with wasm32v1-none and fails E0463 if its std/core is not installed.
    printf "%s\n" "#![no_std]" "pub fn probe() -> u8 { core::num::NonZeroU8::new(1).unwrap().get() }" > /tmp/target-probe.rs
    if ! rustc --edition 2021 --target wasm32v1-none --crate-type lib \
        /tmp/target-probe.rs -o /tmp/target-probe.rlib; then
      printf "wasm32v1-none target is not usable with the pinned toolchain\n" >&2
      exit 1
    fi
    test "$(sha256sum "$cache_file")" = "$cache_hash_before_build"
    printf "release cache miss/hit and explicit build wiring verified for %s\n" "$RELEASE_URL"
  '
