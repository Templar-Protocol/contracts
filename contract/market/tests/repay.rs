//! Ported from `contract/market/tests/repay.rs` onto the gateway harness.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::interest_rate_strategy::InterestRateStrategy;
use templar_gateway_testing::{harness, SandboxHarness};

#[derive(Debug)]
enum RepayAccount {
    Implicit,
    SpecifySelf,
    SpecifyOther,
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn repay(
    #[future(awt)] harness: SandboxHarness,
    #[values(1, 999_999, 1_000_000, 1_000_001, 2_000_000)] repay_amount: u128,
    #[values(
        RepayAccount::Implicit,
        RepayAccount::SpecifySelf,
        RepayAccount::SpecifyOther
    )]
    account: RepayAccount,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let third_party = harness.create_user("third").await?;
    for user in [&supply_user, &borrow_user, &third_party] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 1_000_000).await?;

    let payer = match account {
        RepayAccount::SpecifyOther => &third_party,
        _ => &borrow_user,
    };
    let account_id = match account {
        RepayAccount::Implicit => None,
        _ => Some(borrow_user.0.clone()),
    };

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &payer.0)
        .await?;
    let liability = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .get_total_borrow_asset_liability(),
    );

    harness
        .repay(payer, &market, repay_amount, account_id)
        .await?;

    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &payer.0)
        .await?;
    let liability_after = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .get_total_borrow_asset_liability(),
    );

    if repay_amount <= liability {
        assert_eq!(balance_after, balance_before - repay_amount);
        assert_eq!(liability_after, liability - repay_amount);
    } else {
        assert_eq!(balance_after, balance_before - liability);
        assert_eq!(liability_after, 0);
    }

    Ok(())
}
