//! Ported from `contract/market/tests/collateral.rs` onto the gateway harness.
//!
//! `concurrent_collateral_withdrawals_conserve_deposit` restores the original
//! `collateral_withdrawal`'s coverage: many withdrawals in flight at once,
//! driving repeated `collateral_asset_in_flight` initial/final transitions,
//! including the failure-recovery path when the destination is unregistered.
//!
//! The original raced the withdrawals against the unregistration and asserted
//! "at least one failed" — sound but timing-dependent. Here the two outcomes are
//! separated into deterministic phases: a batch that all succeed (registered)
//! then a batch that all recover (unregistered). The conservation invariant
//! (`withdrawn + remaining == deposited`, nothing lost) is checked via the final
//! deposit, which holds regardless of how the concurrent callbacks interleave.

use std::sync::Arc;

use anyhow::{Context, Result};
use rstest::rstest;
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};
use templar_gateway_types::{ManagedAccountId, OperationStatus};
use tokio::task::JoinSet;

const CONCURRENCY: usize = 30;
const CHUNK: u128 = 5;

async fn collateral_deposit(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<u128> {
    Ok(u128::from(
        harness
            .get_borrow_position(market, &user.0)
            .await?
            .context("borrow position missing")?
            .collateral_asset_deposit,
    ))
}

/// Fire `CONCURRENCY` collateral withdrawals of `CHUNK` each, all in flight at
/// once, and wait for every one to settle (each is a `try_` — a blocked
/// withdrawal reports top-level success while the market recovers the deposit).
async fn withdraw_concurrently(
    harness: Arc<SandboxHarness>,
    market: DeployedMarket,
    user: ManagedAccountId,
) -> Result<()> {
    let mut set = JoinSet::new();
    for _ in 0..CONCURRENCY {
        let harness = Arc::clone(&harness);
        let market = market.clone();
        let user = user.clone();
        set.spawn(async move {
            harness
                .try_withdraw_collateral(&user, &market, CHUNK)
                .await
                .map(|_| ())
        });
    }
    while let Some(joined) = set.join_next().await {
        joined??;
    }
    Ok(())
}

#[rstest]
#[tokio::test]
async fn concurrent_collateral_withdrawals_conserve_deposit(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&borrow_user, &market).await?;

    let deposited = CONCURRENCY as u128 * CHUNK * 2;
    harness
        .collateralize(&borrow_user, &market, deposited)
        .await?;

    let harness = Arc::new(harness);

    // Phase 1: while registered, every concurrent withdrawal succeeds. This
    // drives `CONCURRENCY` overlapping in-flight initial/final(success)
    // transitions; the deposit drops by exactly the amount withdrawn.
    withdraw_concurrently(Arc::clone(&harness), market.clone(), borrow_user.clone()).await?;
    let after_success = collateral_deposit(&harness, &market, &borrow_user).await?;
    assert_eq!(
        after_success,
        deposited - CONCURRENCY as u128 * CHUNK,
        "every registered concurrent withdrawal should move its collateral out",
    );

    // Unregister from the collateral token: the market can no longer return
    // collateral, so each withdrawal's transfer fails and the callback recovers.
    harness
        .storage_unregister(&borrow_user, &market.collateral_ft_id, true)
        .await?;

    // Phase 2: another concurrent batch, all now blocked. This drives
    // `CONCURRENCY` overlapping in-flight initial/final(failure→recover)
    // transitions; no collateral is lost, so the deposit is unchanged.
    withdraw_concurrently(Arc::clone(&harness), market.clone(), borrow_user.clone()).await?;
    let after_blocked = collateral_deposit(&harness, &market, &borrow_user).await?;
    assert_eq!(
        after_blocked, after_success,
        "collateral must not be lost when withdrawals to an unregistered account are blocked",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn excessive_collateral_withdrawal_is_rejected(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let borrow_user_1 = harness.create_user("borrow1").await?;
    let borrow_user_2 = harness.create_user("borrow2").await?;
    harness.fund_user(&borrow_user_1, &market).await?;
    harness.fund_user(&borrow_user_2, &market).await?;

    harness
        .collateralize(&borrow_user_1, &market, 1_000_000)
        .await?;
    harness
        .collateralize(&borrow_user_2, &market, 1_000_000)
        .await?;

    // Withdrawing more collateral than deposited must be rejected (an unsigned
    // underflow), leaving the deposit intact.
    let result = harness
        .try_withdraw_collateral(&borrow_user_1, &market, 1_000_000 + 1)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains("attempt to subtract with overflow"),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );

    let deposit = harness
        .get_borrow_position(&market, &borrow_user_1.0)
        .await?
        .context("borrow position missing")?
        .collateral_asset_deposit;
    assert_eq!(u128::from(deposit), 1_000_000);

    Ok(())
}
