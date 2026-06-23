//! Ported from `contract/market/tests/storage_management.rs`.
//!
//! The original asserts that supplying from an account not registered on the
//! market panics ("is not registered"). The gateway `supply` op auto-registers
//! the signer, so to exercise the contract's own requirement we bypass it with a
//! raw `ft_transfer_call` carrying the `Supply` message. The same *failure
//! condition* surfaces as an effect: the market rejects the deposit inside
//! `ft_on_transfer`, the FT refunds, and no supply position is created.

use anyhow::Result;
use rstest::rstest;
use templar_common::market::DepositMsg;
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn supply_requires_market_registration(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    // `fund_user` registers the account on the tokens and mints it a balance, but
    // does not register it on the market — that registration is what the gateway
    // `supply` op would add, and what we deliberately skip here.
    let user = harness.create_user("supply").await?;
    harness.fund_user(&user, &market).await?;

    let balance_before = harness.ft_balance_of(&market.borrow_ft_id, &user.0).await?;
    harness
        .ft_transfer_call(
            &user,
            &market.borrow_ft_id,
            &market.market_id,
            1000,
            serde_json::to_string(&DepositMsg::Supply)?,
        )
        .await?;

    assert!(
        harness
            .get_supply_position(&market, &user.0)
            .await?
            .is_none(),
        "an unregistered account must not get a supply position",
    );
    assert_eq!(
        harness.ft_balance_of(&market.borrow_ft_id, &user.0).await?,
        balance_before,
        "the rejected deposit must be refunded",
    );

    Ok(())
}
