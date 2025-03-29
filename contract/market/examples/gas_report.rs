#![allow(clippy::unwrap_used)]

use near_sdk::{json_types::U64, Gas};
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, number::Decimal,
    time_chunk::TimeChunkConfiguration,
};
use test_utils::{setup_everything, SetupEverything};

#[allow(clippy::unwrap_used)]
#[tokio::main]
async fn main() {
    const ITERATIONS: usize = 128;

    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        borrow_user_2,
        ..
    } = setup_everything(|c| {
        c.borrow_interest_rate_strategy =
            InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        c.borrow_origination_fee = Fee::zero();
        c.time_chunk_configuration = TimeChunkConfiguration::BlockHeight { divisor: U64(1) };
    })
    .await;

    c.supply(&supply_user, 120_000).await;
    let harvest_yield_0 = c.harvest_yield_execution(&supply_user, true).await;
    let snapshot_count_before = c.list_snapshots(None, None).await.len();
    c.collateralize(&borrow_user, 2000).await;
    c.collateralize(&borrow_user_2, 2000).await;

    c.borrow(&borrow_user_2, 1000).await;
    let apply_interest_0 = c.apply_interest(&borrow_user_2, None).await;

    for _ in 0..ITERATIONS {
        c.borrow(&borrow_user, 1000).await;
        c.repay(&borrow_user, 1100).await;
    }

    let apply_interest_max = c.apply_interest(&borrow_user_2, None).await;
    let harvest_yield_max = c.harvest_yield_execution(&supply_user, true).await;

    let snapshot_count_after = c.list_snapshots(None, None).await.len();
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

    println!("**Gas Report**");
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
