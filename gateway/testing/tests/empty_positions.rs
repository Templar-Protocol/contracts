//! Ported from `contract/market/tests/empty_positions.rs` onto the gateway
//! harness. (The old test re-registered market storage by hand before
//! re-supplying; the gateway `supply` op does that registration itself.)

use anyhow::Result;
use rstest::rstest;
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn empty_positions_are_removed(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness.supply(&supply_user, &market, 1000).await?;
    assert!(harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .is_some());

    harness
        .create_supply_withdrawal_request(&supply_user, &market, 1000)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert!(harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .is_none());

    // Deposit a little bit more again. A full withdrawal refunds only the
    // position's storage (not its snapshots'), so top up before re-supplying;
    // then exercise a collateral round-trip.
    harness
        .storage_deposit_min(&supply_user, &market.market_id)
        .await?;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;

    harness.collateralize(&borrow_user, &market, 2000).await?;
    assert!(harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .is_some());
    harness
        .withdraw_collateral(&borrow_user, &market, 2000)
        .await?;
    assert!(harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .is_none());

    harness
        .create_supply_withdrawal_request(&supply_user, &market, 1000)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert!(harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .is_none());

    Ok(())
}
