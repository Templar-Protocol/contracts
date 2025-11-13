#![allow(clippy::pedantic)]

use near_sdk::{json_types::U128, Gas};
use rand::Rng as _;
use test_utils::{setup_test, ContractController};

#[tokio::main]
async fn main() {
    const ITERATIONS: usize = 128;
    let worker = near_workspaces::sandbox().await.unwrap();

    setup_test!(
        worker
        extract(vault, c, vault_curator)
        accounts(user1, user2, user3)
    );

    vault.init_account(&user1).await;
    vault.init_account(&user2).await;
    vault.init_account(&user3).await;

    let max = c.borrow_asset.balance_of(user1.id()).await;
    let g = || rand::thread_rng().gen_range(0..=max);

    let weights = vec![(c.market.contract().id().clone(), U128(1))];
    let user1_amount = max / ITERATIONS as u128;

    // Run supplies concurrently.
    let mut supply_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        supply_gas_average += vault
            .supply(&user1, user1_amount)
            .await
            .total_gas_burnt
            .as_gas() as f64
            / ITERATIONS as f64;
    }

    let mut allocation_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        let allocation_gas = vault
            .allocate(&vault_curator, weights.clone(), Some(U128(user1_amount)))
            .await
            .total_gas_burnt
            .as_gas() as f64;
        allocation_gas_average += allocation_gas / ITERATIONS as f64;
    }

    // Supply to vault
    let user2_amount = g();
    vault.supply(&user2, user2_amount).await;

    let user3_amount = g();

    // Submitting a smaller gas limit will not require a timelock
    let submit_cap_gas = vault
        .submit_cap(
            &vault_curator,
            c.market.contract().id().clone(),
            U128(user3_amount),
        )
        .await
        .total_gas_burnt
        .as_gas() as f64;

    vault.supply(&user3, user3_amount).await;

    let mut withdraw_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        withdraw_gas_average += vault
            .withdraw(&user2, U128(1), None)
            .await
            .total_gas_burnt
            .as_gas() as f64
            / ITERATIONS as f64;
    }

    let withdraw_route = vec![c.market.contract().id().clone()];

    let mut execute_withdraw_gas_average = 0f64;
    for _ in 0..ITERATIONS {
        let execute_gas = vault
            .execute_next_withdrawal(&vault_curator, withdraw_route.clone())
            .await
            .total_gas_burnt
            .as_gas() as f64;
        execute_withdraw_gas_average += execute_gas / ITERATIONS as f64;
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
}
