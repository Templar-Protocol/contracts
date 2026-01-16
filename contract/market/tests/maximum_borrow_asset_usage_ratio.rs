use near_sandbox::Sandbox;
use rstest::rstest;

use templar_common::number::Decimal;
use test_utils::*;

#[rstest]
#[case(1)]
#[case(50)]
#[case(99)]
#[case(100)]
#[tokio::test]
async fn borrow_within_maximum_usage_ratio(#[future(awt)] worker: Sandbox, #[case] percent: u16) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_asset_maximum_usage_ratio = Decimal::from(percent) / 100u32;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 2000),
    );

    let balance_before = c.borrow_asset.balance_of(borrow_user.id()).await;
    let amount = u128::from(percent) * 10 - 1;
    c.borrow(&borrow_user, amount).await;
    let balance_after = c.borrow_asset.balance_of(borrow_user.id()).await;

    assert_eq!(balance_before + amount, balance_after);
    assert_eq!(
        c.get_borrow_position(borrow_user.id())
            .await
            .unwrap()
            .get_borrow_asset_principal(),
        amount.into(),
    );
}

#[rstest]
#[case(1)]
#[case(50)]
#[case(99)]
#[case(100)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Insufficient borrow asset available"]
async fn borrow_exceeds_maximum_usage_ratio(#[future(awt)] worker: Sandbox, #[case] percent: u16) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_asset_maximum_usage_ratio = Decimal::from(percent) / 100u32;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 2000),
    );

    c.borrow(&borrow_user, u128::from(percent) * 10 + 1).await;
}
