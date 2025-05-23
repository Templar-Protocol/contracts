use rstest::rstest;
use templar_common::{dec, fee::Fee, number::Decimal};
use test_utils::*;

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1"), dec!("1"))]
#[case(dec!("1"), dec!("1.1"))]
#[case(dec!("1"), dec!("2"))]
#[case(dec!("1"), dec!("5"))]
#[tokio::test]
async fn success_above_minimum_initial_collateral_ratio(
    #[case] minimum: Decimal,
    #[case] initial: Decimal,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr = minimum;
            c.borrow_mcr_initial = initial;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(
            &borrow_user,
            (1000u32 * initial + Decimal::ONE).to_u128_ceil().unwrap(),
        ),
    );

    let balance_before = c.borrow_asset.ft_balance_of(borrow_user.id()).await.0;
    c.borrow(&borrow_user, 1000).await;
    let balance_after = c.borrow_asset.ft_balance_of(borrow_user.id()).await.0;

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
#[case(dec!("1"), dec!("1"))]
#[case(dec!("1"), dec!("1.1"))]
#[case(dec!("1"), dec!("2"))]
#[case(dec!("1"), dec!("5"))]
#[tokio::test]
#[should_panic = "Smart contract panicked: New position must exceed initial minimum collateral ratio"]
async fn fail_below_minimum_initial_collateral_ratio(
    #[case] minimum: Decimal,
    #[case] initial: Decimal,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr = minimum;
            c.borrow_mcr_initial = initial;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(
            &borrow_user,
            (1000u32 * initial).to_u128_floor().unwrap() - 1,
        ),
    );

    c.borrow(&borrow_user, 1000).await;
}

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1"), dec!("1.1"))]
#[case(dec!("1.5"), dec!("2"))]
#[case(dec!("1.5"), dec!("5"))]
#[tokio::test]
async fn not_in_liquidation_if_below_minimum_initial_collateral_ratio(
    #[case] minimum: Decimal,
    #[case] initial: Decimal,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr = minimum;
            c.borrow_mcr_initial = initial;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(
            &borrow_user,
            (1000u32 * initial + Decimal::ONE).to_u128_ceil().unwrap(),
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
