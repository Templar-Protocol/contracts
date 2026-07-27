#!/usr/bin/env bash
# Verify the contract artifact catalog is self-consistent:
#   - no artifact claims a release for a version its crate never reached. The
#     reverse — source ahead of the newest release — is normal: unreleased work
#     is *meant* to run ahead, and a bump alone asserts nothing.
#   - each artifact's release list is well-formed: no duplicate versions,
#     64-char digests, and strictly increasing version numbers.
#   - mocks have no releases.
#
# Every check is pure and in-memory: the catalog is data, so nothing here needs
# the network or a warm artifact cache.
#
# Whether a release's bytes match what the source actually compiles to is a
# different question, answered on release tags by
# .github/workflows/release-artifacts.yml.
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
# The whole suite, not a name filter: the catalog invariants live in several
# tests and a filter silently skips any newly added one. Every feature, for the
# same reason — a binary gated behind one takes its tests with it.
cargo test -p templar-contract-artifacts --features fetch,workspace-loader,clap \
    -- --include-ignored --nocapture
