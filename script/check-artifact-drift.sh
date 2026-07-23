#!/usr/bin/env bash
# Verify the checked-in contract artifact catalog is self-consistent:
#   - every release blob in res/near/<target>/<version>/ hashes to the `sha256`
#     pinned for that release in ids.rs (historical releases included — they are
#     immutable, so any change to one is a mistake)
#   - the NEWEST catalogued release of each artifact matches its Cargo.toml
#     version (a bump with no cut blob fails here — run `just artifact-release`)
#   - each artifact's release list is well-formed: non-empty, no duplicate
#     versions, 64-char digests
#   - res/near and the catalog agree: no orphaned directories, no phantom entries
#
# These are pure, in-memory checks — no contract builds, no Docker, seconds.
# Reproducibility (do the bytes match what the source actually compiles to?) is
# verified separately, on release tags, by .github/workflows/release-artifacts.yml.
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
# The whole suite, not a name filter: the catalog invariants live in several
# tests and a filter silently skips any newly added one.
cargo test -p templar-contract-artifacts --features embedded-wasm,workspace-loader \
    -- --include-ignored --nocapture
