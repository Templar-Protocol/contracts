//! Restores the original's coverage: many supplier positions driven through the
//! withdrawal queue *concurrently*, and every fully-principal-withdrawn supplier
//! still correctly represented in the listing afterwards. A supply position is
//! removed only when both its principal and its accrued yield are zero, so each
//! supplier first earns origination-fee yield (paid by the borrowers) and then
//! withdraws only its principal — the position survives with nonzero yield and
//! must remain listed even though the queue processed the withdrawals under
//! concurrent execution.
//!
//! Determinism: the concurrency is in the queue phase (requests created and
//! executors fired concurrently), then a bounded sequential drain guarantees the
//! queue empties before the listing is asserted. The amounts are chosen so every
//! supplier's yield share is comfortably nonzero (no rounding to zero).

use std::sync::Arc;

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::interest_rate_strategy::InterestRateStrategy;
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};
use templar_gateway_types::ManagedAccountId;
use tokio::task::JoinSet;

const SUPPLIERS: usize = 6;
const BORROWERS: usize = 6;
const SUPPLY: u128 = 100_000;
const COLLATERAL: u128 = 200_000;
const BORROW: u128 = 50_000;

async fn supply_position(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<templar_common::supply::SupplyPosition> {
    harness
        .get_supply_position(market, &user.0)
        .await?
        .context("supply position missing")
}

#[rstest]
#[tokio::test]
async fn many_accounts_concurrent_withdrawal_queue_churn(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    // Zero interest for clean numbers; keep the default 10% origination fee so
    // borrows credit the suppliers a nonzero yield.
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    // Suppliers supply and activate.
    let mut suppliers = Vec::with_capacity(SUPPLIERS);
    for i in 0..SUPPLIERS {
        let user = harness.create_user(&format!("supply{i}")).await?;
        harness.fund_user(&user, &market).await?;
        harness
            .supply_and_harvest_until_activation(&user, &market, SUPPLY)
            .await?;
        suppliers.push(user);
    }

    // Borrowers borrow (paying the origination fee, which becomes supplier yield)
    // then repay in full, restoring the liquidity the suppliers will withdraw.
    let mut borrowers = Vec::with_capacity(BORROWERS);
    for i in 0..BORROWERS {
        let user = harness.create_user(&format!("borrow{i}")).await?;
        harness.fund_user(&user, &market).await?;
        harness.collateralize(&user, &market, COLLATERAL).await?;
        harness.borrow(&user, &market, BORROW).await?;
        harness
            .repay(&user, &market, BORROW + BORROW / 10, None)
            .await?;
        borrowers.push(user);
    }

    // Realize each supplier's origination-fee yield; assert it is nonzero, so a
    // full principal withdrawal leaves the position in existence (and listed).
    // Record each balance so the returned principal can be checked afterwards.
    let mut balances_before = Vec::with_capacity(SUPPLIERS);
    for user in &suppliers {
        harness
            .harvest_yield(user, &market, Some(user.0.clone()))
            .await?;
        let position = supply_position(&harness, &market, user).await?;
        assert!(
            !position.borrow_asset_yield.get_total().is_zero(),
            "supplier should have accrued nonzero yield",
        );
        balances_before.push(
            harness
                .asset_balance_of(&market.configuration.borrow_asset, &user.0)
                .await?,
        );
    }

    let harness = Arc::new(harness);

    // Concurrently queue every supplier's full principal withdrawal.
    let mut requests = JoinSet::new();
    for user in &suppliers {
        let harness = Arc::clone(&harness);
        let market = market.clone();
        let user = user.clone();
        requests.spawn(async move {
            harness
                .create_supply_withdrawal_request(&user, &market, SUPPLY)
                .await
                .map(|_| ())
        });
    }
    while let Some(joined) = requests.join_next().await {
        joined??;
    }

    // Fire the queue executors concurrently (each signed by a distinct supplier
    // to avoid single-signer nonce serialization) — this is the concurrent queue
    // execution the listing consistency must survive.
    let mut executors = JoinSet::new();
    for user in &suppliers {
        let harness = Arc::clone(&harness);
        let market = market.clone();
        let user = user.clone();
        executors.spawn(async move {
            harness
                .try_execute_next_supply_withdrawal_request(&user, &market, None)
                .await
                .map(|_| ())
        });
    }
    while let Some(joined) = executors.join_next().await {
        joined??;
    }

    // Bounded sequential drain: concurrent executors can no-op against each other,
    // so finish any remainder deterministically before asserting.
    let driver = suppliers[0].clone();
    for _ in 0..=SUPPLIERS {
        if harness
            .supply_withdrawal_queue_status(&market)
            .await?
            .length
            == 0
        {
            break;
        }
        harness
            .try_execute_next_supply_withdrawal_request(&driver, &market, None)
            .await?;
    }
    assert_eq!(
        harness
            .supply_withdrawal_queue_status(&market)
            .await?
            .length,
        0,
        "withdrawal queue should be fully drained",
    );

    // Every supplier got its full principal back, yet its position remains listed
    // (the withdrawal left the residual origination-fee yield in place). This is
    // the invariant the concurrent queue execution must preserve.
    let supply_positions = harness.list_supply_positions(&market).await?;
    assert_eq!(supply_positions.len(), SUPPLIERS);
    for (user, balance_before) in suppliers.iter().zip(&balances_before) {
        let position = supply_positions
            .get(&user.0)
            .with_context(|| format!("fully-withdrawn supply position dropped for {}", user.0))?;
        assert!(
            position.exists(),
            "residual yield keeps the withdrawn position in existence",
        );
        let balance_after = harness
            .asset_balance_of(&market.configuration.borrow_asset, &user.0)
            .await?;
        assert_eq!(
            balance_after,
            balance_before + SUPPLY,
            "supplier should have its full principal returned",
        );
    }

    // Every borrower keeps a (collateralized) position and stays listed.
    let borrow_positions = harness.list_borrow_positions(&market).await?;
    assert_eq!(borrow_positions.len(), BORROWERS);
    for user in &borrowers {
        assert!(
            borrow_positions.contains_key(&user.0),
            "missing borrow position for {}",
            user.0,
        );
    }

    Ok(())
}
