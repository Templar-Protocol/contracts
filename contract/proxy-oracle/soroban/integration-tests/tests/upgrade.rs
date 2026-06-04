#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group L — Upgrade flow.
//!
//! L2 Zero hash is rejected at proposal-create time by `validate_action`,
//!    not at execute time. The wasm-hash check is duplicated on the runtime
//!    too as defense in depth.
//! L3 Direct call from a non-owner is rejected.
//!
//! L1 ("successful upgrade") is not exercised here because soroban-sdk
//! testutils does not bind WASM hashes to deployable code in-process; a true
//! end-to-end upgrade requires either a local-node tier (out of scope) or a
//! second compiled WASM artifact mounted into the test. The ABI path is
//! still covered by L2 + L3 + the runtime's own unit test for the event
//! emission shape.

use soroban_sdk::testutils::{Address as _, BytesN as _};
use soroban_sdk::{Address, BytesN};
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn upgrade_proposal_with_zero_hash_is_rejected_at_create_time() {
    let b = Bootstrap::new();
    let zero = BytesN::<32>::from_array(&b.env, &[0_u8; 32]);
    // `validate_action` runs in `create_proposal`, so the create call is the
    // error site — the proposal never makes it to the pending set.
    let next_id = b.governance.next_proposal_id();
    let result =
        b.governance
            .try_create_proposal(&b.admin, &next_id, &GovernanceAction::Upgrade(zero), &0);
    assert!(result.is_err(), "zero wasm hash must be rejected");
}

#[test]
#[should_panic]
fn direct_runtime_upgrade_by_non_owner_panics() {
    let b = Bootstrap::new();
    let rando = Address::generate(&b.env);
    let dummy_hash = BytesN::<32>::random(&b.env);
    // try_upgrade is the panic-returning try variant; the wrapper will
    // propagate the inner error in soroban-sdk testutils, but
    // `unwrap_err`-style introspection isn't reliable for owner-check
    // panics. Use `should_panic` as the simplest verification.
    b.runtime.upgrade(&dummy_hash, &rando);
}
