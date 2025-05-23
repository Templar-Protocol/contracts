use rstest::rstest;
use test_utils::*;

#[rstest]
#[case(0, &[1], u128::MAX)]
#[case(1, &[1], u128::MAX)]
#[case(10, &[10], 10)]
#[case(0, &[20, 20, 20, 20, 20], 100)]
#[tokio::test]
async fn borrow_within_bounds(
    #[case] minimum: u128,
    #[case] amounts: &[u128],
    #[case] maximum: u128,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_maximum_amount = maximum.into();
            c.borrow_minimum_amount = minimum.into();
        })
    );

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;

    for amount in amounts {
        c.borrow(&borrow_user, *amount).await;
    }
}

#[rstest]
#[case(2, 1, 2)]
#[case(100, 99, 1000)]
#[case(u128::MAX, 1, u128::MAX)]
#[case(1000, 738, u128::MAX)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Borrow amount is smaller than minimum allowed"]
async fn borrow_below_minimum(#[case] minimum: u128, #[case] amount: u128, #[case] maximum: u128) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_maximum_amount = maximum.into();
            c.borrow_minimum_amount = minimum.into();
        })
    );

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, amount).await;
}

#[rstest]
#[case(0, &[2], 1)]
#[case(0, &[1, 1], 1)]
#[case(0, &[1001], 1000)]
#[case(0, &[1000, 1], 1000)]
#[case(1000, &[1001], 1000)]
#[case(500, &[500, 501], 1000)]
#[case(100, &[1001], 500)]
#[case(100, &[100, 100, 100, 100, 100, 100, 100], 500)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Borrow amount is greater than maximum allowed"]
async fn borrow_above_maximum(
    #[case] minimum: u128,
    #[case] amounts: &[u128],
    #[case] maximum: u128,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_maximum_amount = maximum.into();
            c.borrow_minimum_amount = minimum.into();
        })
    );

    c.supply(&supply_user, 10000).await;
    c.collateralize(&borrow_user, 2000).await;

    for amount in amounts {
        c.borrow(&borrow_user, *amount).await;
    }
}
