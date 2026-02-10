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
    let outcome = compute_payout_success_outcome(escrow_shares, expected_assets, settled_assets);
    assert!(matches!(
        outcome,
        PayoutOutcome::Success {
            burn_shares: actual_burn,
            refund_shares: actual_refund
        }
        if actual_burn == burn_shares && actual_refund == refund_shares
    ));
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
    assert!(matches!(
        outcome,
        PayoutOutcome::Failure {
            restore_idle: actual_idle,
            refund_shares: actual_refund
        }
        if actual_idle == restore_idle && actual_refund == refund_shares
    ));
}
