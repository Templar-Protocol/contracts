use std::collections::HashMap;

use near_sdk::{json_types::U64, Gas};
use templar_common::time_chunk::TimeChunkConfiguration;
use test_utils::{setup_everything, SetupEverything};
use tokio::task::JoinSet;

#[allow(clippy::unwrap_used)]
#[tokio::main]
async fn main() {
    const STEP: usize = 10;
    const COUNT: usize = 3;

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
}

async fn harvest_yield_gas(iterations: usize) -> Gas {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.time_chunk_configuration = TimeChunkConfiguration::BlockHeight { divisor: U64(1) };
    })
    .await;

    c.supply(&supply_user, 1200).await;
    c.collateralize(&borrow_user, 2000).await;

    for _ in 0..iterations {
        c.borrow(&borrow_user, 1000).await;
        c.repay(&borrow_user, 1100).await;
    }

    c.harvest_yield_execution(&supply_user, true)
        .await
        .total_gas_burnt
}
