# Vendored fork artifacts

Optimized-wasm copies of the Precept (Templar Blend v2 fork) contracts pinned to
fork commit `54afdae01bb8e484514940229e06460aa3fa6a1f` (2026-09-16). These are
loaded by `src/testutils.rs` via `contractimport!` so the carry-adapter test
harness exercises the real fork pool and pool-factory wasm. (The fork
backstop is exercised through an in-crate `MockBackstop`; see `testutils.rs`.)

| File | SHA-256 |
| --- | --- |
| `pool.wasm` | `2db04d55fa91246f263c7cd98ba9b436a97e5f32218d67a86ea7beca057939fa` |
| `pool_factory.wasm` | `80abaf3c68fd15adade9f82165baa9b50d85800e497c99b45809e1fbd7397de2` |

The same digests are pinned as consts in `src/testutils.rs` and verified
on-chain by `templar_soroban_carry_adapter::testutils::verify_artifact_pins()`
(asserted in `tests/harness_smoke.rs`).

## Build record

The vendored bytes were produced by the fork's own `make build` target
(`cargo rustc --manifest-path=<crate>/Cargo.toml --crate-type=cdylib
--target=wasm32-unknown-unknown --release` + `stellar contract optimize`)
against the fork working tree at `54afdae` (tree verified clean for
`pool/`, `backstop/`, `pool-factory/`; `make build` is the documented
entrypoint, and the fork `rust-toolchain.toml` pins channel 1.81).

## Reproduction caveat (recorded 2026-10-12)

A clean re-run of `make build` from `54afdae` with `CARGO_TARGET_DIR=/tmp/...`
fails on this host under the pinned 1.81 toolchain: dependency `ethnum 1.5.0`
(pulled via `soroban-env-common`, pinned in the fork `Cargo.lock`) hits the
known `mem::transmute(())` const-error —
`cannot transmute between types of different sizes` at
`ethnum-1.5.0/src/error.rs:16`. The failure is in the crate cache/toolchain
combination, not in the fork source; re-vendoring therefore requires fixing
the `ethnum` pin (bump or patch) and regenerating these artifacts, after which
the digests above and the consts in `src/testutils.rs` must be updated together.

Do not re-vendor casually: the harness tests must run against the exact
audited fork bytes; any re-vendoring is a governed change.
