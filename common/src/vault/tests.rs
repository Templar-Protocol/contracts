use super::PendingValue;
use near_sdk::{test_utils::VMContextBuilder, testing_env};

#[test]
fn pending_value_verify_succeeds_after_timelock_maturity() {
    let mut context = VMContextBuilder::new();
    context.block_timestamp(1_000);
    testing_env!(context.build());

    let pending = PendingValue {
        value: "ok",
        valid_at_ns: 1_000,
    };
    pending.verify();
}

#[test]
#[should_panic(expected = "Timelock not elapsed yet")]
fn pending_value_verify_panics_before_timelock_maturity() {
    let mut context = VMContextBuilder::new();
    context.block_timestamp(999);
    testing_env!(context.build());

    let pending = PendingValue {
        value: "blocked",
        valid_at_ns: 1_000,
    };
    pending.verify();
}
