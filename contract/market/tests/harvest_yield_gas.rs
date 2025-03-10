use rstest::rstest;
use test_utils::{setup_everything, SetupEverything, EQUAL_PRICE};

#[rstest]
#[case(0)]
#[case(10)]
// #[case(20)]
// #[case(30)]
// #[case(40)]
#[tokio::test]
async fn harvest_yield_gas(#[case] iterations: usize) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 1200).await;
    c.collateralize(&borrow_user, 2000).await;

    for i in 0..iterations {
        if i % 10 == 0 {
            println!("Iteration {i}...");
        }
        c.borrow(&borrow_user, 1000, EQUAL_PRICE).await;
        c.repay(&borrow_user, 1100).await;
    }

    let r = c.harvest_yield(&supply_user).await;

    println!("Total gas burnt ({iterations}): {}", r.total_gas_burnt);
}
