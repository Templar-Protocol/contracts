#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Gas benchmark for the core vault operations, driven through the in-process
//! gateway [`SandboxHarness`] — the same path the services use. Prints a
//! markdown gas table averaged over many iterations.
//!
//! Run with: `cargo run -p templar-vault-contract --example vault_gas_report`

use anyhow::Result;
use near_sdk::{json_types::U128, Gas};
use templar_common::vault::{AllocationDelta, Delta};
use templar_gateway_testing::SandboxHarness;

#[tokio::main]
async fn main() -> Result<()> {
    // Each iteration is a real on-chain transaction through the gateway, so keep
    // the sample count modest — gas per action is near-deterministic. Raise it
    // for a longer, smoother run.
    const ITERATIONS: usize = 16;

    let harness = SandboxHarness::start().await?;
    let vault = harness.deploy_vault_with_market().await?;

    let user1 = harness.create_user("user1").await?;
    let user2 = harness.create_user("user2").await?;
    let user3 = harness.create_user("user3").await?;
    harness.vault_init_account(&user1, &vault).await?;
    harness.vault_init_account(&user2, &vault).await?;
    harness.vault_init_account(&user3, &vault).await?;

    let max = harness
        .ft_balance_of(&vault.market.borrow_ft_id, &user1.0)
        .await?;
    let market_id = harness
        .vault_market_id_of(&vault.vault_id, &vault.market.market_id)
        .await?
        .expect("market registered");

    let user1_amount = max / ITERATIONS as u128;

    let mut supply_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        let result = harness.vault_supply(&user1, &vault, user1_amount).await?;
        supply_gas_average += harness.operation_gas_burnt(&result) as f64 / ITERATIONS as f64;
    }

    let mut allocation_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        let result = harness
            .vault_allocate(
                &vault.curator,
                &vault,
                AllocationDelta::Supply(Delta::new(market_id, U128(user1_amount))),
            )
            .await?;
        allocation_gas_average += harness.operation_gas_burnt(&result) as f64 / ITERATIONS as f64;
    }

    // Deterministic amounts (the benchmark only needs representative sizes;
    // avoids a `rand` dependency).
    let user2_amount = max / 3;
    harness.vault_supply(&user2, &vault, user2_amount).await?;

    let user3_amount = max / 7;
    // A cap smaller than the current one (u128::MAX) applies without a timelock.
    let submit_cap_result = harness
        .vault_submit_cap(&vault.owner, &vault, &vault.market.market_id, user3_amount)
        .await?;
    let submit_cap_gas = harness.operation_gas_burnt(&submit_cap_result) as f64;

    harness.vault_supply(&user3, &vault, user3_amount).await?;

    let mut withdraw_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        let result = harness.vault_withdraw(&user2, &vault, 1, None).await?;
        withdraw_gas_average += harness.operation_gas_burnt(&result) as f64 / ITERATIONS as f64;
    }

    let mut execute_withdraw_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        let result = harness
            .vault_execute_withdrawal(&vault.curator, &vault, &[vault.market.market_id.clone()])
            .await?;
        execute_withdraw_gas_average +=
            harness.operation_gas_burnt(&result) as f64 / ITERATIONS as f64;
    }

    println!("## Gas Report");
    println!();
    println!("Estimated allocation limit: 0");
    println!();
    println!("### Action Gas Descriptors");
    println!();
    println!("| Action | Gas  |");
    println!("| -----: | ---: |");
    let list = vec![
        ("supply", Gas::from_gas(supply_gas_average as u64)),
        ("allocate", Gas::from_gas(allocation_gas_average as u64)),
        ("withdraw", Gas::from_gas(withdraw_gas_average as u64)),
        (
            "execute withdraw",
            Gas::from_gas(execute_withdraw_gas_average as u64),
        ),
        ("submit_cap", Gas::from_gas(submit_cap_gas as u64)),
    ];
    for (action_label, gas) in list {
        println!("| `{action_label}` | {gas} |");
    }
    println!();
    Ok(())
}
