#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use near_sdk::{json_types::U64, Gas};
use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, number::Decimal,
    time_chunk::TimeChunkConfiguration,
};
use test_utils::{setup_everything, SetupEverything};
use tokio::task::JoinSet;

#[allow(clippy::unwrap_used)]
#[tokio::main]
async fn main() {
    const STEP: usize = 100;
    const COUNT: usize = 4;

    let mut handles = JoinSet::new();

    for i in 0..COUNT {
        handles.spawn(async move {
            let gas = harvest_yield_gas(i * STEP).await;
            eprintln!("Completed {} iterations", i * STEP);
            (i, gas)
        });
    }

    let results = handles
        .join_all()
        .await
        .into_iter()
        .collect::<HashMap<_, _>>();

    println!("**Gas Report**");
    println!();
    println!("`harvest_yield`");
    println!();
    println!("| Iterations | Gas  |");
    println!("| ---------: | ---: |");
    for i in 0..COUNT {
        println!("| {} | {} |", i * STEP, results.get(&i).unwrap());
    }

    // Estimate `snapshot_limit` parameter of `harvest_yield` function that
    // will maximize iterations while safely staying within the maximum gas limit.
    let at_0 = results.get(&0).unwrap();
    let max_snapshots = (COUNT - 1) * STEP;
    let at_max_snapshots = results.get(&(COUNT - 1)).unwrap();
    let snapshot_limit = calculate_snapshot_limit(
        *at_0,
        max_snapshots as u64,
        *at_max_snapshots,
        Gas::from_tgas(285), // Max gas is 300, so this is a bit conservative
    );
    println!();
    println!("Estimated `snapshot_limit`: {snapshot_limit}");
}

fn calculate_snapshot_limit(
    at_0: Gas,
    max_snapshots: u64,
    at_max_snapshots: Gas,
    target_gas: Gas,
) -> u64 {
    (target_gas.as_gas() - at_0.as_gas()) * max_snapshots
        / (at_max_snapshots.as_gas() - at_0.as_gas())
}

async fn harvest_yield_gas(iterations: usize) -> Gas {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.borrow_interest_rate_strategy =
            InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        c.borrow_origination_fee = Fee::zero();
        c.time_chunk_configuration = TimeChunkConfiguration::BlockHeight { divisor: U64(1) };
    })
    .await;

    c.supply(&supply_user, 120_000).await;
    c.collateralize(&borrow_user, 2000).await;

    for _ in 0..iterations {
        c.borrow(&borrow_user, 1000).await;
        c.repay(&borrow_user, 1100).await;
    }

    let r = c.harvest_yield(&supply_user, true).await;

    r.total_gas_burnt
}
