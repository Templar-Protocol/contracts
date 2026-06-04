// The mock oracle's contract entry points take `Env` and `Asset` by value
// (required by `#[contractimpl]`), and the SDK macros expand parameter
// names verbatim — `_timestamp` placeholders in trait methods get reported
// by `clippy::used_underscore_binding`. Both are deliberate.
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]
// Module docs name SDK types like `Sep40Adapter` and `Env` without
// backticks; not worth threading.
#![allow(clippy::doc_markdown)]

//! End-to-end integration tests for the Soroban proxy-oracle contracts.
//!
//! Mirrors the in-process `Env::default()` pattern the vault crate uses for
//! its Blend e2e tests: deploys the real runtime + governance + SEP-40 adapter
//! contracts into a single `Env`, plus a mock upstream SEP-40 price feed,
//! drives them through their generated `*Client` types, and asserts on
//! observable state + emitted events. No live Soroban RPC.
//!
//! Each scenario file under `tests/` is its own integration-test binary; all
//! share the `common` harness re-exported from this crate.

pub mod common;
