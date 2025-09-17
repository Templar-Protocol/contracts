use rstest::rstest;
use templar_common::{dec, fee::Fee, number::Decimal};
use test_utils::*;

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1.000000000000000000000000000001"), dec!("1.000000000000000000000000000001"))]
#[case(dec!("1.00000001"), dec!("1.1"))]
#[case(dec!("1.00000000000000000000000000000000001"), dec!("5"))]
#[tokio::test]
async fn success_above_mcr_maintenance(#[case] liquidation: Decimal, #[case] maintenance: Decimal) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = liquidation;
            c.borrow_mcr_maintenance = maintenance;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(
            &borrow_user,
            (1000u32 * maintenance + Decimal::ONE)
                .to_u128_ceil()
                .unwrap(),
        ),
    );

    let balance_before = c.borrow_asset.balance_of(borrow_user.id()).await;
    c.borrow(&borrow_user, 1000).await;
    let balance_after = c.borrow_asset.balance_of(borrow_user.id()).await;

    assert_eq!(balance_before + 1000, balance_after);
    assert_eq!(
        u128::from(
            c.get_borrow_position(borrow_user.id())
                .await
                .unwrap()
                .get_borrow_asset_principal()
        ),
        1000
    );
}

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1.001"), dec!("1.001"))]
#[case(dec!("1.001"), dec!("1.1"))]
#[case(dec!("1.001"), dec!("2"))]
#[case(dec!("1.001"), dec!("5"))]
#[tokio::test]
#[should_panic = "Smart contract panicked: Borrow position must be healthy after borrow"]
async fn fail_below_mcr_maintenance(#[case] liquidation: Decimal, #[case] maintenance: Decimal) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = liquidation;
            c.borrow_mcr_maintenance = maintenance;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(
            &borrow_user,
            (1000u32 * maintenance).to_u128_floor().unwrap() - 1,
        ),
    );

    c.borrow(&borrow_user, 1000).await;
}

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1.001"), dec!("1.1"))]
#[case(dec!("1.5"), dec!("2"))]
#[case(dec!("1.5"), dec!("5"))]
#[tokio::test]
async fn not_in_liquidation_if_below_mcr_maintenance(
    #[case] liquidation: Decimal,
    #[case] maintenance: Decimal,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = liquidation;
            c.borrow_mcr_maintenance = maintenance;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(
            &borrow_user,
            (1000u32 * maintenance + Decimal::ONE)
                .to_u128_ceil()
                .unwrap(),
        ),
    );

    c.borrow(&borrow_user, 1000).await;

    c.set_collateral_asset_price(0.99).await;

    let borrow_status = c
        .get_borrow_status(borrow_user.id(), c.get_prices().await)
        .await
        .unwrap();

    assert!(!borrow_status.is_liquidation(), "Borrow should not be in liquidation when collateralization ratio is below minimum INITIAL if it is still above the minimum for liquidation.");
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Borrow position must be healthy after collateral withdrawal"]
async fn withdraw_collateral_below_mcr_maintenance() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = dec!("1.2");
            c.borrow_mcr_maintenance = dec!("1.5");
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, 1500),
    );

    c.borrow(&borrow_user, 1000).await;

    c.withdraw_collateral(&borrow_user, 1).await;
}
