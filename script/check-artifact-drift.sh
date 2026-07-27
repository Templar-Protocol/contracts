#!/usr/bin/env bash
# Verify the contract artifact catalog is self-consistent:
#   - the NEWEST catalogued release of each artifact matches its Cargo.toml
#     version. Bumping a contract's version is a release claim, so a bump with
#     no release entry fails here — in the developer's own PR, before merge.
#   - each artifact's release list is well-formed: no duplicate versions,
#     64-char digests, and at most one pin-pending release (which must be the
#     newest, and must not be a legacy one).
#   - mocks have no releases; real contracts have at least one.
#
# Plus, while contract/artifacts/res/ still exists (see the burn-in note in
# src/catalog.rs), that the bytes the fetch path returns are byte-identical to
# the blobs checked in. That comparison needs a warm artifact cache or network
# access; everything else is pure and in-memory.
#
# Whether a release's bytes match what the source actually compiles to is a
# different question, answered on release tags by
# .github/workflows/release-artifacts.yml.
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
# The whole suite, not a name filter: the catalog invariants live in several
# tests and a filter silently skips any newly added one.
cargo test -p templar-contract-artifacts --features fetch,workspace-loader \
    -- --include-ignored --nocapture
