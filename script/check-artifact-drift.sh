#!/usr/bin/env bash
# Verify the contract artifact catalog is self-consistent, purely in memory —
# no network, no warm cache:
#   - no artifact claims a release its crate's version never reached (the
#     reverse is normal: unreleased work is *meant* to run ahead)
#   - release lists are well-formed: unique versions, 64-char digests, ascending
#   - mocks have no releases, and scaffolding crates are excluded from releases
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
# The whole suite, not a name filter: the catalog invariants live in several
# tests and a filter silently skips any newly added one. Every feature, for the
# same reason — a binary gated behind one takes its tests with it.
cargo test -p templar-contract-artifacts --features fetch,workspace-loader,clap \
    -- --include-ignored --nocapture
