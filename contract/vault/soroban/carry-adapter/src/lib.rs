#![no_std]

//! Templar carry-adapter market adapter (RE7 carry-trade yield vault).
//!
//! The adapter is a single Soroban market adapter for the Templar vault: it
//! supplies the vault asset as collateral to a Precept (Templar Blend v2 fork)
//! pool, borrows a borrow asset against it, and deploys the borrowed funds
//! into a yield leg (a plain `Supply` position on a second pool). On
//! withdrawal it repays then withdraws collateral, and it marks NAV as
//! `C + conv(Y - D)` with a capped oracle-free band (see the carry-adapter
//! plan / ADR for the accounting model).
//!
//! Layout:
//!
//! - [`math`]: pure fixed-point helpers for the position/NAV accounting.
//! - [`contract`]: the `CarryAdapterContract` body (vault ABI + admin views).
//! - [`oracle_client`] / [`pool_ops`]: thin external-ABI wrappers.
//! - `testutils`: the fork-backed test harness ([`testutils::CarryFixture`],
//!   sha256-pinned vendored fork artifacts, and a settable SEP-40 mock
//!   oracle). The harness compiles for host test builds and dependents via
//!   the `testutils` feature.

// The `#[contractimpl]`/`contractargs` macros re-emit the constructor with
// its full flat ABI arg list, which this lint cannot be silenced on locally.
#![allow(clippy::too_many_arguments)]

/// Pure position/NAV accounting helpers shared by the adapter body and its
/// proptests.
pub mod math;

#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

pub mod contract;
#[cfg(test)]
mod contract_tests;
mod oracle_client;
mod pool_ops;

#[cfg(any(test, feature = "testutils"))]
extern crate std;
