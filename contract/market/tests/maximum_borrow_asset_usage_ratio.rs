use rstest::rstest;
use test_utils::*;

use templar_common::number::Decimal;

#[rstest]
#[case(1)]
#[case(50)]
#[case(99)]
#[case(100)]
#[tokio::test]
async fn borrow_within_maximum_usage_ratio(#[case] percent: u16) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_asset_maximum_usage_ratio = Decimal::from(percent) / 100u32;
        })
    );

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, u128::from(percent) * 10 - 1).await;
}

#[rstest]
#[case(1)]
#[case(50)]
#[case(99)]
#[case(100)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Insufficient borrow asset available"]
async fn borrow_exceeds_maximum_usage_ratio(#[case] percent: u16) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_asset_maximum_usage_ratio = Decimal::from(percent) / 100u32;
        })
    );

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, u128::from(percent) * 10 + 1).await;
}
