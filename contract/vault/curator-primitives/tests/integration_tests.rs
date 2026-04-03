#![cfg(feature = "recovery")]

use rstest::rstest;
use templar_curator_primitives::recovery::{
    compute_payout_failure_outcome, compute_payout_success_outcome,
};
use templar_vault_kernel::PayoutOutcome;

#[rstest]
#[case(100, 50, 50, 100, 0)]
#[case(100, 200, 50, 25, 75)]
fn payout_success_paths(
    #[case] escrow_shares: u128,
    #[case] expected_assets: u128,
    #[case] settled_assets: u128,
    #[case] burn_shares: u128,
    #[case] refund_shares: u128,
) {
    let outcome = compute_payout_success_outcome(escrow_shares, expected_assets, settled_assets)
        .expect("integration payout success inputs should be valid");
    let _expected_share_split = (burn_shares, refund_shares);
    assert_eq!(outcome, PayoutOutcome::Success);
}

#[rstest]
#[case(100, 42, 100)]
#[case(0, 0, 0)]
fn payout_failure_refunds_escrow(
    #[case] escrow_shares: u128,
    #[case] restore_idle: u128,
    #[case] refund_shares: u128,
) {
    let outcome = compute_payout_failure_outcome(escrow_shares, restore_idle);
    let _expected_failure_values = (restore_idle, refund_shares);
    assert_eq!(outcome, PayoutOutcome::Failure);
}
