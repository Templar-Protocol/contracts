#!/usr/bin/env bash
set -ex

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

cd "$ROOT_DIR"
cargo run -p templar-contract-artifacts --features workspace-loader,clap --bin prebuild-test-contracts -- --workspace-root "$ROOT_DIR" "$@" 1>&2

# External fixture release/checksum pins are separate from Templar contract IDs.
# Full prebuilds and --check include them; an explicit workspace --artifact
# selection (or release --resolve) keeps its existing narrow behavior.
_sputnik_fixture=1
_sputnik_check=()
for _prebuild_arg in "$@"; do
    case "$_prebuild_arg" in
        --artifact|--artifact=*|--resolve|--resolve=*|--help|-h|--version|-V)
            _sputnik_fixture=0
            ;;
        --check)
            _sputnik_check=(--check)
            ;;
    esac
done
if [ "$_sputnik_fixture" -eq 1 ]; then
    python3 "$ROOT_DIR/script/fetch-sputnik-fixture.py" "${_sputnik_check[@]}"
fi
unset _sputnik_fixture _sputnik_check _prebuild_arg
