//! Ported from `contract/market/tests/happy_path.rs` (the NEP-141 variant).
//!
//! The original is parametrized over NEP-141 vs NEP-245 (multi-token) assets;
//! the MT variants need MT-contract deploy support in the harness and are not
//! ported here. Exercises the full lifecycle including the 8/1/1 yield split.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{
    borrow::BorrowStatus, dec, interest_rate_strategy::InterestRateStrategy, market::YieldWeights,
    Decimal,
};
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn test_happy(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let protocol = harness.create_user("protocol").await?;
    let insurance = harness.create_user("insurance").await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;

    let protocol_id = protocol.0.clone();
    let insurance_id = insurance.0.clone();
    let market = harness
        .deploy_full_market_with(move |c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
            c.yield_weights = YieldWeights::new_with_supply_weight(8)
                .with_static(protocol_id, 1)
                .with_static(insurance_id, 1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    for user in [&protocol, &insurance, &supply_user, &borrow_user] {
        harness.fund_user(user, &market).await?;
    }

    assert!(market
        .configuration
        .borrow_mcr_liquidation
        .near_equal(dec!("1.2")));

    // A single snapshot is generated on init, with no activity.
    let snapshots = harness.list_finalized_snapshots(&market).await?;
    assert_eq!(
        snapshots.len(),
        1,
        "should generate a single snapshot on init"
    );
    assert!(snapshots[0].yield_distribution.is_zero());
    assert!(snapshots[0].borrow_asset_deposited_active.is_zero());
    assert!(snapshots[0].borrow_asset_borrowed.is_zero());

    // Step 1: supply, then activate.
    harness.supply(&supply_user, &market, 1100).await?;
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .total_incoming()
        ),
        1100,
    );
    while !harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .context("supply position missing")?
        .get_deposit()
        .incoming
        .is_empty()
    {
        harness
            .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
            .await?;
    }
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .get_deposit()
                .active
        ),
        1100,
    );

    // Step 2: collateralize, and confirm healthy with nothing borrowed.
    harness.collateralize(&borrow_user, &market, 2000).await?;
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .collateral_asset_deposit
        ),
        2000,
    );
    let prices = harness.get_oracle_prices(&market).await?;
    assert_eq!(
        harness
            .get_borrow_status(&market, &borrow_user.0, prices)
            .await?
            .context("borrow status missing")?,
        BorrowStatus::Healthy,
    );

    // Step 3: borrow (1000 + 100 origination fee).
    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    harness.borrow(&borrow_user, &market, 1000).await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
            .await?,
        balance_before + 1000,
    );
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_total_borrow_asset_liability()
        ),
        1100,
    );

    // Step 4: repay in full.
    harness.repay(&borrow_user, &market, 1100, None).await?;
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_total_borrow_asset_liability()
        ),
        0,
    );

    // The 100 origination fee is split 8/1/1 across supply / protocol / insurance.

    // Supply yield: 80.
    harness
        .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
        .await?;
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .borrow_asset_yield
                .get_total()
        ),
        80,
    );
    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 80)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before + 80,
    );

    // Withdraw the supplied principal (1100); the position then closes.
    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 1100)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before + 1100,
    );
    assert!(harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .is_none());

    // Protocol and insurance static yield: 10 each.
    for recipient in [&protocol, &insurance] {
        harness
            .accumulate_static_yield(recipient, &market, Some(recipient.0.clone()), None)
            .await?;
        assert_eq!(harness.static_yield_total(&market, &recipient.0).await?, 10);
        let balance_before = harness
            .ft_balance_of(&market.borrow_ft_id, &recipient.0)
            .await?;
        harness
            .withdraw_static_yield(recipient, &market, None)
            .await?;
        assert_eq!(
            harness
                .ft_balance_of(&market.borrow_ft_id, &recipient.0)
                .await?,
            balance_before + 10,
        );
    }

    // Borrower withdraws all collateral; the borrow position closes.
    let balance_before = harness
        .ft_balance_of(&market.collateral_ft_id, &borrow_user.0)
        .await?;
    harness
        .withdraw_collateral(&borrow_user, &market, 2000)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &borrow_user.0)
            .await?,
        balance_before + 2000,
    );
    assert!(harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .is_none());

    Ok(())
}
