use rstest::rstest;
use templar_common::{fee::Fee, interest_rate_strategy::InterestRateStrategy, number::Decimal};
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
            c.borrow_range = (minimum, Some(maximum)).try_into().unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 2000),
    );

    for amount in amounts {
        c.borrow(&borrow_user, *amount).await;
    }

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(
        borrow_position.get_borrow_asset_principal(),
        amounts.iter().sum::<u128>().into(),
    );
}

#[rstest]
#[case(2, 1, 2)]
#[case(100, 99, 1000)]
#[case(u128::MAX, 1, u128::MAX)]
#[case(1000, 738, u128::MAX)]
#[tokio::test]
#[should_panic = "Smart contract panicked: New borrow position is outside of allowable range"]
async fn borrow_below_minimum(#[case] minimum: u128, #[case] amount: u128, #[case] maximum: u128) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_range = (minimum, Some(maximum)).try_into().unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 2000),
    );

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
#[should_panic = "Smart contract panicked: New borrow position is outside of allowable range"]
async fn borrow_above_maximum(
    #[case] minimum: u128,
    #[case] amounts: &[u128],
    #[case] maximum: u128,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_range = (minimum, Some(maximum)).try_into().unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, 2000),
    );

    for amount in amounts {
        c.borrow(&borrow_user, *amount).await;
    }
}

#[rstest]
#[tokio::test]
async fn withdraw_below_minimum() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_range = (10, None).try_into().unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy = InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 2000),
    );
    c.borrow(&borrow_user, 100).await;
    let borrow_position_before = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(
        borrow_position_before.get_total_borrow_asset_liability(),
        100.into()
    );
    c.repay(&borrow_user, 91).await;
    let borrow_position_after = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(
        borrow_position_after.get_total_borrow_asset_liability(),
        10.into(),
    );
}
