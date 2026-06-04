#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Groups C/D/E — Circuit breaker behavior.
//!
//! C1 StepwiseChange trips on a single-step spike.
//! C2 The same breaker with `is_enforced=false` observes but does not block.
//! D1 MonotonicRun trips on N consecutive directional moves.
//! E1 WindowedChangeDelta trips on slow drift across windows.
//! E2 Rearm recovers a tripped breaker after the cool-down.
//! E3 RemoveBreaker invalidates the cache.

use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    CircuitBreakerConfig, MonotonicRunConfig, RearmConfig, SetEnforcedConfig, SorobanDecimal,
    StepwiseChangeConfig, WindowedChangeDeltaConfig,
};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::{ledger, Bootstrap};

fn half(b: &Bootstrap) -> SorobanDecimal {
    SorobanDecimal::from_decimal(&b.env, Decimal::ONE_HALF)
}

fn one_percent(b: &Bootstrap) -> SorobanDecimal {
    SorobanDecimal::from_decimal(&b.env, templar_primitives::dec!("0.01"))
}

fn configure_with_breaker(b: &Bootstrap, history_len: u32, breaker: CircuitBreakerConfig) {
    b.configure_default_feed();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, history_len),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::AddBreaker(b.asset_btc.clone(), breaker),
    );
}

#[test]
fn stepwise_trips_on_single_step_spike() {
    let b = Bootstrap::new();
    configure_with_breaker(
        &b,
        8,
        CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
            max_relative_change: half(&b),
        }),
    );

    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let accepted = b.refresh_one(&b.asset_btc);
    assert!(matches!(accepted, RefreshStatus::Accepted(_)));

    ledger::advance_secs(&b.env, 1);
    b.push_upstream_price(&b.asset_btc, 10_000_000_000, 101);
    let blocked = b.refresh_one(&b.asset_btc);
    assert!(matches!(blocked, RefreshStatus::Blocked(_)));

    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(view.is_blocking);
    assert!(!view.is_manually_tripped);

    // SEP-40 consumers see no price.
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());
}

#[test]
fn stepwise_not_enforced_does_not_block() {
    let b = Bootstrap::new();
    configure_with_breaker(
        &b,
        8,
        CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
            max_relative_change: half(&b),
        }),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetEnforced(
            b.asset_btc.clone(),
            0,
            SetEnforcedConfig { is_enforced: false },
        ),
    );

    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));

    ledger::advance_secs(&b.env, 1);
    b.push_upstream_price(&b.asset_btc, 10_000_000_000, 101);
    // Even with the spike, refresh is Accepted because the breaker is observing
    // only.
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
fn monotonic_trips_on_sustained_directional_moves() {
    let b = Bootstrap::new();
    // 1% min step, 3-step streak — 4th matching step trips.
    configure_with_breaker(
        &b,
        16,
        CircuitBreakerConfig::MonotonicRun(MonotonicRunConfig {
            max_streak: 3,
            min_relative_step_change: one_percent(&b),
        }),
    );

    let mut price: i128 = 1_000_000_000;
    let mut ts: u64 = 100;
    for _ in 0..4 {
        b.push_upstream_price(&b.asset_btc, price, ts);
        let _ = b.refresh_one(&b.asset_btc);
        ledger::advance_secs(&b.env, 1);
        // +5% per step keeps each individual move above the 1% threshold but
        // well below the StepwiseChange logic isn't installed here.
        price = price * 105 / 100;
        ts += 1;
    }

    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(view.is_blocking);
}

#[test]
fn windowed_change_delta_trips_on_accelerated_change() {
    let b = Bootstrap::new();
    // 2-entry windows, 1 lookback window, 10% max delta. The breaker compares
    // the current window's signed relative change against the previous
    // window's; only the *delta* between them matters. A flat first window
    // followed by a spiked second window crosses the threshold.
    configure_with_breaker(
        &b,
        16,
        CircuitBreakerConfig::WindowedChangeDelta(WindowedChangeDeltaConfig {
            window_len: 2,
            lookback_windows: 1,
            max_relative_change_delta: SorobanDecimal::from_decimal(
                &b.env,
                templar_primitives::dec!("0.10"),
            ),
        }),
    );

    // Window 0 (positions 0,1): flat at 1_000_000_000 → 0% change.
    // Window 1 (positions 2,3): 1_000_000_000 → 1_500_000_000 → 50% change.
    // Delta = |50% - 0%| = 50% > 10% → trips on the 4th observation.
    let samples = [
        (1_000_000_000_i128, 100_u64),
        (1_000_000_000, 101),
        (1_000_000_000, 102),
        (1_500_000_000, 103),
    ];
    let mut tripped = false;
    for (i, (price, ts)) in samples.iter().enumerate() {
        b.push_upstream_price(&b.asset_btc, *price, *ts);
        if matches!(b.refresh_one(&b.asset_btc), RefreshStatus::Blocked(_)) {
            tripped = true;
            assert_eq!(i, 3, "should trip on the 4th observation, not earlier");
            break;
        }
        ledger::advance_secs(&b.env, 1);
    }
    assert!(tripped, "WindowedChangeDelta should have tripped");
}

#[test]
fn rearm_recovers_a_tripped_breaker() {
    let b = Bootstrap::new();
    configure_with_breaker(
        &b,
        8,
        CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
            max_relative_change: half(&b),
        }),
    );

    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);
    ledger::advance_secs(&b.env, 1);
    b.push_upstream_price(&b.asset_btc, 10_000_000_000, 101);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Blocked(_)
    ));

    // Rearm with a 60s grace then drive the breaker forward.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::Rearm(
            b.asset_btc.clone(),
            0,
            RearmConfig {
                armed_after_secs: 60,
                accepted_history_source_code: 1, // Observed
            },
        ),
    );

    // Advance past the grace window and refresh at a stable price.
    ledger::advance_secs(&b.env, 120);
    let ts = b.env.ledger().timestamp();
    b.push_upstream_price(&b.asset_btc, 10_000_000_000, ts);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
fn remove_breaker_invalidates_the_cache() {
    let b = Bootstrap::new();
    configure_with_breaker(
        &b,
        8,
        CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
            max_relative_change: half(&b),
        }),
    );

    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);
    assert!(b.runtime.get_cached(&b.asset_btc).is_some());

    b.submit_and_execute(
        &b.admin,
        GovernanceAction::RemoveBreaker(b.asset_btc.clone(), 0),
    );
    // After RemoveBreaker (which mutates the breaker set), the cache is wiped.
    assert!(b.runtime.get_cached(&b.asset_btc).is_none());
    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert_eq!(view.breaker_count, 0);
}
