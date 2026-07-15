//! Ported from `contract/market/tests/interest_rate.rs`. The *exact* interest
//! arithmetic is covered by the deterministic, node-free `templar-common` unit
//! test `calculate_interest_two_snapshots_exact`. This integration test covers
//! what genuinely needs a node: every interest-rate strategy actually accrues
//! through the deployed contract, and neither applying borrow interest nor
//! harvesting supply yield more frequently changes the total owed/earned.
//!
//! Time is advanced with `fast_forward` (deterministic) rather than wall-clock
//! sleeps, so the frequency-neutrality checks cannot flake.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, Decimal};
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};
use templar_gateway_types::ManagedAccountId;

/// A borrower's *realized* interest. Read realized-only (not pending) so the
/// figure is frozen at the last snapshot rather than growing with the sandbox's
/// continuous block production between reads — otherwise the later of two
/// sequential reads always looks larger. Callers realize every position to the
/// same snapshot (a final `apply_interest`) before comparing.
async fn realized_interest(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<u128> {
    Ok(u128::from(
        harness
            .get_borrow_position(market, &user.0)
            .await?
            .context("borrow position missing")?
            .interest
            .get_total(),
    ))
}

/// A supplier's *realized* yield — realized-only for the same reason as
/// [`realized_interest`].
async fn realized_yield(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<u128> {
    Ok(u128::from(
        harness
            .get_supply_position(market, &user.0)
            .await?
            .context("supply position missing")?
            .borrow_asset_yield
            .get_total(),
    ))
}

/// Repay `user`'s entire liability *including* interest still pending across
/// snapshots (read separately from the materialized liability), with a 10%
/// overpayment, and assert the position clears. Guards that the repay path
/// settles pending interest, not only the already-materialized portion.
async fn repay_in_full(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<()> {
    let position = harness
        .get_borrow_position(market, &user.0)
        .await?
        .context("borrow position missing")?;
    let pending = harness
        .get_borrow_position_pending_interest(market, &user.0)
        .await?;
    assert!(
        !pending.is_zero(),
        "expected unrealized interest so the repay actually settles a pending amount",
    );
    let owed = u128::from(position.get_total_borrow_asset_liability() + pending);
    harness.repay(user, market, owed * 110 / 100, None).await?;
    assert!(
        harness
            .get_borrow_position(market, &user.0)
            .await?
            .is_none_or(|p| p.get_total_borrow_asset_liability().is_zero()),
        "borrow should be fully repaid (incl. pending interest) after a 10% overpayment",
    );
    Ok(())
}

#[rstest]
#[case(10_000_000, InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap())]
#[case(10_000_000, InterestRateStrategy::linear(dec!("10"), dec!("500")).unwrap())]
#[case(5_000_000,
    InterestRateStrategy::piecewise(Decimal::ZERO, dec!("0.09"), dec!("35"), dec!("600")).unwrap())]
#[case(5_000_000,
    InterestRateStrategy::exponential2(dec!("5"), dec!("800"), dec!("6")).unwrap())]
#[tokio::test]
async fn interest_accrues_per_strategy_and_frequency_is_neutral(
    #[future(awt)] harness: SandboxHarness,
    #[case] principal: u128,
    #[case] strategy: InterestRateStrategy,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with({
            let strategy = strategy.clone();
            move |c| {
                c.borrow_origination_fee = Fee::zero();
                c.borrow_interest_rate_strategy = strategy;
            }
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    // `end`/`lazy` realize only at the end; `freq`/`eager` do so every chunk.
    let supply_end = harness.create_user("supply-end").await?;
    let supply_freq = harness.create_user("supply-freq").await?;
    let borrow_lazy = harness.create_user("borrow-lazy").await?;
    let borrow_eager = harness.create_user("borrow-eager").await?;
    for user in [&supply_end, &supply_freq, &borrow_lazy, &borrow_eager] {
        harness.fund_user(user, &market).await?;
    }

    let supply_amount = principal * 5;
    harness
        .supply_and_harvest_until_activation(&supply_end, &market, supply_amount)
        .await?;
    harness
        .supply_and_harvest_until_activation(&supply_freq, &market, supply_amount)
        .await?;
    harness
        .collateralize(&borrow_lazy, &market, supply_amount)
        .await?;
    harness
        .collateralize(&borrow_eager, &market, supply_amount)
        .await?;
    // Two borrowers each take `principal` of the `10 * principal` supplied: a
    // ~0.2 utilization, the point the original evaluated its rate bounds at.
    harness.borrow(&borrow_lazy, &market, principal).await?;
    harness.borrow(&borrow_eager, &market, principal).await?;

    // Advance time in chunks; `eager` applies interest and `freq` harvests each
    // chunk, while `lazy`/`end` wait for the end.
    for _ in 0..4 {
        harness.fast_forward(60).await?;
        harness
            .apply_interest(&borrow_eager, &market, Some(borrow_eager.0.clone()), None)
            .await?;
        harness
            .harvest_yield(&supply_freq, &market, Some(supply_freq.0.clone()))
            .await?;
    }

    // Realize every position before reading, so the frequency comparison is
    // between frozen figures. Concurrently, and each signed by a distinct
    // account: they land in the same time-chunk and thus realize to the *same*
    // snapshot — otherwise, at the extreme flat-1000 rate, the ~1s between two
    // sequential realizations is itself hundreds of units of interest.
    tokio::try_join!(
        harness.apply_interest(&borrow_eager, &market, Some(borrow_eager.0.clone()), None),
        harness.apply_interest(&borrow_lazy, &market, Some(borrow_lazy.0.clone()), None),
        harness.harvest_yield(&supply_freq, &market, Some(supply_freq.0.clone())),
        harness.harvest_yield(&supply_end, &market, Some(supply_end.0.clone())),
    )?;

    let eager = realized_interest(&harness, &market, &borrow_eager).await?;
    let lazy = realized_interest(&harness, &market, &borrow_lazy).await?;
    let yield_freq = realized_yield(&harness, &market, &supply_freq).await?;
    let yield_end = realized_yield(&harness, &market, &supply_end).await?;

    // Every strategy actually accrues through the deployed contract.
    assert!(
        eager > 0 && lazy > 0,
        "strategy accrued no borrow interest (eager {eager}, lazy {lazy})",
    );
    assert!(
        yield_end > 0 && yield_freq > 0,
        "strategy accrued no supply yield (end {yield_end}, freq {yield_freq})",
    );

    // Interest is simple, not compounding, so applying it more or less often must
    // not change the total a borrower owes — beyond per-application rounding,
    // which the eager borrower incurs on every one of its realizations. That
    // rounding scales with the rate, so allow 3%: enough to absorb it at the
    // extreme flat-1000 (case 1) rate, still far tighter than the tens of percent
    // a genuine compounding dependence would produce.
    assert!(
        eager.abs_diff(lazy) <= lazy * 3 / 100 + 200,
        "application frequency changed interest (eager {eager} vs lazy {lazy})",
    );

    // Likewise, harvesting only realizes pending yield, so harvest frequency must
    // not change a supplier's total yield.
    assert!(
        yield_freq.abs_diff(yield_end) <= yield_end / 100 + 100,
        "harvest frequency changed supply yield (end {yield_end} vs freq {yield_freq})",
    );

    // The deployed market must select the rate from the *configured* strategy at
    // the realized utilization, not a fixed rate or the strategy at the wrong
    // utilization. Each finalized snapshot records
    // `interest_rate = strategy.at(usage_ratio(active, borrowed))`; recomputing
    // that from the snapshot's own recorded active/borrowed must reproduce it.
    let snapshots = harness.list_finalized_snapshots(&market).await?;
    let borrowing = snapshots
        .iter()
        .find(|s| u128::from(s.borrow_asset_borrowed) == 2 * principal)
        .context("no finalized snapshot captured both borrows active")?;
    let utilization = Decimal::from(borrowing.borrow_asset_borrowed)
        / Decimal::from(borrowing.borrow_asset_deposited_active);
    assert!(
        borrowing.interest_rate.near_equal(strategy.at(utilization)),
        "snapshot rate {:?} should equal the configured strategy at its utilization ({:?})",
        borrowing.interest_rate,
        strategy.at(utilization),
    );

    // Restore the pending-interest repayment path: advance time and finalize a
    // fresh snapshot via a supplier harvest that touches neither borrower, so each
    // now carries interest pending across snapshots; then repay liability +
    // pending and assert the debt clears.
    harness.fast_forward(60).await?;
    harness
        .harvest_yield(&supply_freq, &market, Some(supply_freq.0.clone()))
        .await?;
    repay_in_full(&harness, &market, &borrow_lazy).await?;
    repay_in_full(&harness, &market, &borrow_eager).await?;

    Ok(())
}
