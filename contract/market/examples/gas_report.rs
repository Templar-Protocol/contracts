#![allow(clippy::unwrap_used, clippy::wildcard_imports)]

use near_sdk::Gas;
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, market::HarvestYieldMode,
    time_chunk::TimeChunkConfiguration, Decimal,
};
use test_utils::*;

#[allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
#[tokio::main]
async fn main() {
    const ITERATIONS: usize = 128;

    let worker = worker().await;

    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, borrow_user_2, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1);
        })
    );

    let e = c
        .supply_and_harvest_until_activation(&supply_user, 120_000)
        .await;
    let supply_gas = e.total_gas_burnt;
    let harvest_yield_0 = c
        .harvest_yield_exec(&supply_user, None, Some(HarvestYieldMode::Default))
        .await;
    let snapshot_count_before = c.list_finalized_snapshots(None, None).await.len();

    let (a, b) = tokio::join!(
        async {
            c.collateralize(&borrow_user, 2000)
                .await
                .total_gas_burnt
                .as_gas() as f64
                / 2f64
        },
        async {
            c.collateralize(&borrow_user_2, 2000)
                .await
                .total_gas_burnt
                .as_gas() as f64
                / 2f64
        },
    );
    let collateralize_gas_average = a + b;

    c.borrow(&borrow_user_2, 1000).await;
    let apply_interest_0 = c.apply_interest(&borrow_user_2, None, None).await;

    let mut borrow_gas_average = 0f64;
    let mut repay_gas_average = 0f64;

    for _ in 0..ITERATIONS {
        let e = c.borrow(&borrow_user, 1000).await;
        borrow_gas_average += e.total_gas_burnt.as_gas() as f64 / ITERATIONS as f64;
        let e = c.repay(&borrow_user, None, 1100).await;
        repay_gas_average += e.total_gas_burnt.as_gas() as f64 / ITERATIONS as f64;
    }

    let apply_interest_max = c.apply_interest(&borrow_user_2, None, None).await;
    let harvest_yield_max = c
        .harvest_yield_exec(&supply_user, None, Some(HarvestYieldMode::Default))
        .await;

    c.repay(&borrow_user_2, None, 1100).await;

    let snapshot_count_after = c.list_finalized_snapshots(None, None).await.len();
    let snapshot_count = snapshot_count_after - snapshot_count_before;
    eprintln!("Snapshot count: {snapshot_count}");
    let target_gas = Gas::from_tgas(285); // Max gas is 300, so this is a bit conservative

    let harvest_yield_snapshot_limit = calculate_snapshot_limit(
        harvest_yield_0.total_gas_burnt,
        snapshot_count as u64,
        harvest_yield_max.total_gas_burnt,
        target_gas,
    );

    let apply_interest_snapshot_limit = calculate_snapshot_limit(
        apply_interest_0.total_gas_burnt,
        snapshot_count as u64,
        apply_interest_max.total_gas_burnt,
        target_gas,
    );

    let (a, b) = tokio::join!(
        async {
            c.withdraw_collateral(&borrow_user, 1000)
                .await
                .total_gas_burnt
                .as_gas() as f64
                / 2f64
        },
        async {
            c.withdraw_collateral(&borrow_user_2, 10)
                .await
                .total_gas_burnt
                .as_gas() as f64
                / 2f64
        },
    );
    let withdraw_collateral_gas_average = a + b;

    let e = c
        .create_supply_withdrawal_request(&supply_user, 120_000)
        .await;
    let create_supply_withdrawal_gas = e.total_gas_burnt;
    let e = c
        .execute_next_supply_withdrawal_request_exec(&supply_user, None)
        .await;
    let execute_supply_withdrawal_gas = e.total_gas_burnt;

    println!("## Gas Report");
    println!();
    println!("### Snapshot Limits");
    println!();
    println!("`harvest_yield`");
    println!();
    println!("| Iterations | Gas  |");
    println!("| ---------: | ---: |");
    println!("| 0 | {} |", harvest_yield_0.total_gas_burnt);
    println!(
        "| {snapshot_count} | {} |",
        harvest_yield_max.total_gas_burnt
    );
    println!();
    println!("Estimated snapshot limit: {harvest_yield_snapshot_limit}");
    println!();
    println!("`apply_interest`");
    println!();
    println!("| Iterations | Gas  |");
    println!("| ---------: | ---: |");
    println!("| 0 | {} |", apply_interest_0.total_gas_burnt);
    println!(
        "| {snapshot_count} | {} |",
        apply_interest_max.total_gas_burnt
    );
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
