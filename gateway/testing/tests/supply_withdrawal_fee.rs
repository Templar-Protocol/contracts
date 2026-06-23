//! Ported from `contract/market/tests/supply_withdrawal_fee.rs`. The original
//! slept real wall-clock to age the deposit past the fee window; here the expiry
//! case advances time with `fast_forward`. The `TimeBasedFee` math itself is
//! covered by pure tests in `templar-common`.

use anyhow::Result;
use near_sdk::json_types::U64;
use near_token::NearToken;
use rstest::rstest;
use templar_common::{
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    market::YieldWeights,
};
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};

const THIRTY_DAYS_MS: u64 = 1000 * 60 * 60 * 24 * 30;

/// Register `account` for storage on the market so it can hold a static-yield
/// record.
async fn register_on_market(
    harness: &SandboxHarness,
    account: &templar_gateway_types::ManagedAccountId,
    market: &DeployedMarket,
) -> Result<()> {
    harness
        .storage_deposit(account, &market.market_id, NearToken::from_millinear(50))
        .await?;
    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn fee_applied_within_window(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let protocol = harness.create_user("protocol").await?;
    let protocol_id = protocol.0.clone();
    let market = harness
        .deploy_full_market_with(move |c| {
            c.supply_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_fee = TimeBasedFee {
                fee: Fee::Flat(100.into()),
                duration: U64(THIRTY_DAYS_MS),
                behavior: TimeBasedFeeFunction::Fixed,
            };
            c.protocol_account_id = protocol_id.clone();
            c.yield_weights = YieldWeights::new_with_supply_weight(8).with_static(protocol_id, 1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&protocol, &market).await?;
    harness.fund_user(&supply_user, &market).await?;
    register_on_market(&harness, &protocol, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    let protocol_yield_before = harness.static_yield_total(&market, &protocol.0).await?;

    // Withdrawing well within the 30-day window charges the flat fee.
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 1000)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;

    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    let protocol_yield_after = harness.static_yield_total(&market, &protocol.0).await?;

    assert_eq!(
        balance_after,
        balance_before + 900,
        "the 100 early-withdrawal fee should be deducted",
    );
    assert_eq!(
        protocol_yield_after,
        protocol_yield_before + 100,
        "the fee should be credited to the protocol account",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn no_fee_after_window(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let protocol = harness.create_user("protocol").await?;
    let protocol_id = protocol.0.clone();
    let market = harness
        .deploy_full_market_with(move |c| {
            c.supply_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_fee = TimeBasedFee {
                fee: Fee::Flat(100.into()),
                duration: U64(1000), // 1 second
                behavior: TimeBasedFeeFunction::Fixed,
            };
            c.protocol_account_id = protocol_id.clone();
            c.yield_weights = YieldWeights::new_with_supply_weight(8).with_static(protocol_id, 1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&protocol, &market).await?;
    harness.fund_user(&supply_user, &market).await?;
    register_on_market(&harness, &protocol, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    let protocol_yield_before = harness.static_yield_total(&market, &protocol.0).await?;

    // Age the deposit past the 1s fee window.
    harness.fast_forward(200).await?;

    harness
        .create_supply_withdrawal_request(&supply_user, &market, 1000)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;

    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    let protocol_yield_after = harness.static_yield_total(&market, &protocol.0).await?;

    assert_eq!(
        balance_after,
        balance_before + 1000,
        "no fee should apply after the window expires",
    );
    assert_eq!(
        protocol_yield_after, protocol_yield_before,
        "no fee should be credited after the window expires",
    );

    Ok(())
}
