use templar_curator_primitives::recovery::{
    compute_payout_failure_outcome, compute_payout_success_outcome,
};
use templar_vault_kernel::PayoutOutcome;

#[test]
fn payout_success_full_burn() {
    let outcome = compute_payout_success_outcome(100, 50, 50);
    assert!(matches!(
        outcome,
        PayoutOutcome::Success {
            burn_shares: 100,
            refund_shares: 0
        }
    ));
}

#[test]
fn payout_success_partial_burn() {
    let outcome = compute_payout_success_outcome(100, 200, 50);
    assert!(matches!(
        outcome,
        PayoutOutcome::Success {
            burn_shares: 25,
            refund_shares: 75
        }
    ));
}

#[test]
fn payout_failure_refund_all() {
    let outcome = compute_payout_failure_outcome(100, 42);
    assert!(matches!(
        outcome,
        PayoutOutcome::Failure {
            restore_idle: 42,
            refund_shares: 100
        }
    ));
}
