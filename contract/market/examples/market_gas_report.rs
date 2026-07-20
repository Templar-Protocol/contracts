#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Gas benchmark for the core market operations, driven through the in-process
//! gateway [`SandboxHarness`] — the same path the services use. Prints a
//! markdown gas table averaged over many iterations.
//!
//! Run with: `cargo run -p templar-market-contract --example market_gas_report`

use anyhow::{Context, Result};
use near_sdk::Gas;
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, market::HarvestYieldMode,
    time_chunk::TimeChunkConfiguration, Decimal,
};
use templar_gateway_testing::{DeployedMarket, ManagedAccountId, SandboxHarness};

#[tokio::main]
async fn main() -> Result<()> {
    // Each iteration is a real on-chain transaction through the gateway, so keep
    // the sample count modest — gas per action is near-deterministic. Raise it
    // for a longer, smoother run.
    const ITERATIONS: usize = 16;

    let harness = SandboxHarness::start().await?;
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let borrow_user = harness.create_user("borrow").await?;
    let borrow_user_2 = harness.create_user("borrow2").await?;
    let supply_user = harness.create_user("supply").await?;
    for user in [&borrow_user, &borrow_user_2, &supply_user] {
        harness.fund_user(user, &market).await?;
    }

    // Supply, measure its gas, then harvest until the deposit is activated.
    let supply_result = harness.supply(&supply_user, &market, 120_000).await?;
    let supply_gas = Gas::from_gas(harness.operation_gas_burnt(&supply_result));
    activate(&harness, &market, &supply_user).await?;

    let harvest_yield_0 = harness
        .harvest_yield_with_mode(&supply_user, &market, None, Some(HarvestYieldMode::Default))
        .await?;
    let harvest_yield_0_gas = Gas::from_gas(harness.operation_gas_burnt(&harvest_yield_0));
    let snapshot_count_before = harness.list_finalized_snapshots(&market).await?.len();

    let collateralize_a = {
        let e = harness.collateralize(&borrow_user, &market, 2000).await?;
        harness.operation_gas_burnt(&e) as f64
    };
    let collateralize_b = {
        let e = harness.collateralize(&borrow_user_2, &market, 2000).await?;
        harness.operation_gas_burnt(&e) as f64
    };
    let collateralize_gas_average = (collateralize_a + collateralize_b) / 2f64;

    harness.borrow(&borrow_user_2, &market, 1000).await?;
    let apply_interest_0 = harness
        .apply_interest(&borrow_user_2, &market, None, None)
        .await?;
    let apply_interest_0_gas = Gas::from_gas(harness.operation_gas_burnt(&apply_interest_0));

    let mut borrow_gas_average = 0f64;
    let mut repay_gas_average = 0f64;

    for _ in 0..ITERATIONS {
        let e = harness.borrow(&borrow_user, &market, 1000).await?;
        borrow_gas_average += harness.operation_gas_burnt(&e) as f64 / ITERATIONS as f64;
        let e = harness.repay(&borrow_user, &market, 1100, None).await?;
        repay_gas_average += harness.operation_gas_burnt(&e) as f64 / ITERATIONS as f64;
    }

    let apply_interest_max = harness
        .apply_interest(&borrow_user_2, &market, None, None)
        .await?;
    let apply_interest_max_gas = Gas::from_gas(harness.operation_gas_burnt(&apply_interest_max));
    let harvest_yield_max = harness
        .harvest_yield_with_mode(&supply_user, &market, None, Some(HarvestYieldMode::Default))
        .await?;
    let harvest_yield_max_gas = Gas::from_gas(harness.operation_gas_burnt(&harvest_yield_max));

    harness.repay(&borrow_user_2, &market, 1100, None).await?;

    let snapshot_count_after = harness.list_finalized_snapshots(&market).await?.len();
    let snapshot_count = snapshot_count_after - snapshot_count_before;
    eprintln!("Snapshot count: {snapshot_count}");
    let target_gas = Gas::from_tgas(285); // Max gas is 300, so this is a bit conservative

    let harvest_yield_snapshot_limit = calculate_snapshot_limit(
        harvest_yield_0_gas,
        snapshot_count as u64,
        harvest_yield_max_gas,
        target_gas,
    );

    let apply_interest_snapshot_limit = calculate_snapshot_limit(
        apply_interest_0_gas,
        snapshot_count as u64,
        apply_interest_max_gas,
        target_gas,
    );

    let withdraw_a = {
        let e = harness
            .withdraw_collateral(&borrow_user, &market, 1000)
            .await?;
        harness.operation_gas_burnt(&e) as f64
    };
    let withdraw_b = {
        let e = harness
            .withdraw_collateral(&borrow_user_2, &market, 10)
            .await?;
        harness.operation_gas_burnt(&e) as f64
    };
    let withdraw_collateral_gas_average = (withdraw_a + withdraw_b) / 2f64;

    let e = harness
        .create_supply_withdrawal_request(&supply_user, &market, 120_000)
        .await?;
    let create_supply_withdrawal_gas = Gas::from_gas(harness.operation_gas_burnt(&e));
    let e = harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    let execute_supply_withdrawal_gas = Gas::from_gas(harness.operation_gas_burnt(&e));

    println!("## Gas Report");
    println!();
    println!("### Snapshot Limits");
    println!();
    println!("`harvest_yield`");
    println!();
    println!("| Iterations | Gas  |");
    println!("| ---------: | ---: |");
    println!("| 0 | {harvest_yield_0_gas} |");
    println!("| {snapshot_count} | {harvest_yield_max_gas} |");
    println!();
    println!("Estimated snapshot limit: {harvest_yield_snapshot_limit}");
    println!();
    println!("`apply_interest`");
    println!();
    println!("| Iterations | Gas  |");
    println!("| ---------: | ---: |");
    println!("| 0 | {apply_interest_0_gas} |");
    println!("| {snapshot_count} | {apply_interest_max_gas} |");
    println!();
    println!("Estimated snapshot limit: {apply_interest_snapshot_limit}");
    println!();
    println!("### Action Gas Descriptors");
    println!();
    println!("| Action | Gas  |");
    println!("| -----: | ---: |");
    let list = vec![
        (
            "collateralize",
            Gas::from_gas(collateralize_gas_average as u64),
        ),
        (
            "withdraw_collateral",
            Gas::from_gas(withdraw_collateral_gas_average as u64),
        ),
        ("borrow", Gas::from_gas(borrow_gas_average as u64)),
        ("repay", Gas::from_gas(repay_gas_average as u64)),
        ("supply", supply_gas),
        (
            "create_supply_withdrawal_request",
            create_supply_withdrawal_gas,
        ),
        (
            "execute_next_supply_withdrawal_request",
            execute_supply_withdrawal_gas,
        ),
    ];
    for (action_label, gas) in list {
        println!("| `{action_label}` | {gas} |");
    }
    println!();
    Ok(())
}

/// Harvest the market until `user`'s supply deposit is fully activated (no
/// longer in the `incoming` bucket).
async fn activate(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<()> {
    for _ in 0..1000 {
        if harness
            .get_supply_position(market, &user.0)
            .await?
            .context("supply position missing after supply")?
            .get_deposit()
            .incoming
            .is_empty()
        {
            return Ok(());
        }
        harness
            .harvest_yield(user, market, Some(user.0.clone()))
            .await?;
    }
    anyhow::bail!("supply deposit did not activate after 1000 harvests")
}

/// Estimate `snapshot_limit` that will maximize iterations while safely
/// staying within the gas limit.
fn calculate_snapshot_limit(
    at_0: Gas,
    max_snapshots: u64,
    at_max_snapshots: Gas,
    target_gas: Gas,
) -> u64 {
    (target_gas.as_gas() - at_0.as_gas()) * max_snapshots
        / (at_max_snapshots.as_gas() - at_0.as_gas())
}
