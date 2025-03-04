use test_utils::{setup_everything, SetupEverything, EQUAL_PRICE};

#[tokio::test]
async fn harvest_yield_gas() {
    const ITERATIONS: usize = 10;

    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.borrow_asset_transfer(&supply_user, borrow_user.id(), 100 * ITERATIONS as u128)
        .await;

    c.supply(&supply_user, 1200).await;
    c.collateralize(&borrow_user, 2000).await;

    for _ in 0..ITERATIONS {
        c.borrow(&borrow_user, 1000, EQUAL_PRICE).await;
        c.repay(&borrow_user, 1100).await;
    }

    let r = c.harvest_yield(&supply_user).await;
    println!("{r:#?}");

    println!("Total gas burnt: {}", r.total_gas_burnt);
    println!("Tokens burnt on outcome: {}", r.outcome().tokens_burnt);
    println!("Gas burnt on outcome: {}", r.outcome().gas_burnt);
    println!(
        "Sum of gas on outcomes: {}",
        near_sdk::Gas::from_gas(r.outcomes().iter().map(|o| o.gas_burnt.as_gas()).sum()),
    );
}
