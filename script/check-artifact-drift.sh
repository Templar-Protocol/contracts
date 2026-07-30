#!/usr/bin/env bash
# Verify the contract artifact catalog is self-consistent: no network, no warm
# cache, no contract builds. Each release file's own shape is validated earlier,
# by contract/artifacts/build.rs. See contract/artifacts/README.md.
set -ex

SCRIPT_DIR=$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"
# The whole suite, not a name filter: the catalog invariants live in several
# tests and a filter silently skips any newly added one. Every feature, for the
# same reason — a binary gated behind one takes its tests with it.
cargo test -p templar-contract-artifacts --features fetch,workspace-loader,clap \
    -- --include-ignored --nocapture
