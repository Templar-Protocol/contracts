use std::time::Duration;

use rstest::rstest;

use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::HarvestYieldMode,
    number::Decimal, oracle::pyth,
};
use test_utils::*;
use tokio::time::Instant;

#[tokio::test]
async fn successful_liquidation_totally_underwater() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 500),
    );

    c.borrow(&borrow_user, 300).await;

    // value of collateral will go 500->250
    // collateralization: 250/300 ~= 83%
    // which is bad debt (<100%).

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    c.set_collateral_asset_price(0.5).await;
    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        300, // this is fmv (i.e. NOT what a real liquidator would do to purchase bad debt)
    )
    .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        500,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        300,
        "Liquidation should transfer correct amount of tokens",
    );
}

// Caveat to this test: Make sure that the yield distribution value is
// divisible by 10 for easy maths.
#[rstest]
#[case(110, 5000, 2450, 50, 2500)]
#[case(120, 1250, 1000, 88, 1100)] // fmv
#[case(120, 1250, 1000, 88, 1070)] // liquidator spread of ~2.7%
#[tokio::test]
async fn successful_liquidation_good_debt_under_mcr(
    #[case] mcr: u16,
    #[case] collateral_amount: u128,
    #[case] borrow_amount: u128,
    #[case] collateral_asset_price_pct: u128,
    #[case] liquidation_amount: u128,
) {
    setup_test!(
        extract(c, protocol_yield_user, insurance_yield_user)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr = Decimal::from(mcr) / 100u32;
            c.borrow_mcr_initial = Decimal::from(mcr) / 100u32;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, collateral_amount),
    );
    c.borrow(&borrow_user, borrow_amount).await;

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    c.set_collateral_asset_price(
        (Decimal::from(collateral_asset_price_pct) / 100u32).to_f64_lossy(),
    )
    .await;
    c.liquidate(&liquidator_user, borrow_user.id(), liquidation_amount)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        collateral_amount,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        liquidation_amount,
        "Liquidation should transfer correct amount of tokens",
    );

    let yield_amount = liquidation_amount - borrow_amount;

    tokio::join!(
        async {
            c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
                .await;
            let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
            assert_eq!(
                u128::from(supply_position.borrow_asset_yield.get_total()),
                yield_amount * 8 / 10,
            );
        },
        async {
            let protocol_yield = c.get_static_yield(protocol_yield_user.id()).await.unwrap();
            assert_eq!(u128::from(protocol_yield.borrow_asset), yield_amount / 10);
        },
        async {
            let insurance_yield = c.get_static_yield(insurance_yield_user.id()).await.unwrap();
            assert_eq!(u128::from(insurance_yield.borrow_asset), yield_amount / 10);
        },
    );
}

#[rstest]
#[case(120, 5, 0)]
#[case(120, 5, 2)]
#[case(120, 5, 5)]
#[case(110, 2, 1)]
#[case(150, 33, 32)]
#[tokio::test]
async fn successful_liquidation_with_spread(
    #[case] mcr: u16,
    #[case] maximum_spread_pct: u16,
    #[case] spread_pct: u16,
) {
    assert!(spread_pct <= maximum_spread_pct);

    let liquidation_maximum_spread: Decimal = Decimal::from(maximum_spread_pct) / 100u32;
    let target_spread: Decimal = Decimal::from(spread_pct) / 100u32;
    let mcr: Decimal = Decimal::from(mcr) / 100u32;

    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr = mcr;
            c.borrow_mcr_initial = mcr;
            c.liquidation_maximum_spread = liquidation_maximum_spread;
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, 2000), // 2:1 collateralization
    );
    c.borrow(&borrow_user, 1000).await;

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    let collateral_asset_price: Decimal = mcr /
        201u32 * 100u32 // 2:1 collateralization + a bit to ensure we're under MCR
        ;

    let liquidation_amount = (collateral_asset_price * (1u32 - target_spread) * 2000u32)
        .to_u128_ceil()
        .unwrap();

    c.set_collateral_asset_price(collateral_asset_price.to_f64_lossy())
        .await;
    c.liquidate(&liquidator_user, borrow_user.id(), liquidation_amount)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        2000,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        liquidation_amount,
        "Liquidation should transfer correct amount of tokens",
    );
}

#[tokio::test]
async fn fail_liquidation_too_little_attached() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 500),
    );
    c.borrow(&borrow_user, 300).await;

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    c.set_collateral_asset_price(0.5).await;
    c.liquidate(&liquidator_user, borrow_user.id(), 150).await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_before, collateral_balance_after,
        "Liquidator should not obtain any additional collateral from a rejected liquidation attempt",
    );
    assert_eq!(
        borrow_balance_before, borrow_balance_after,
        "Liquidator should be refunded for a rejected liquidation attempt",
    );

    // ensure borrow position remains unchanged
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(
        u128::from(borrow_position.get_borrow_asset_principal()),
        300,
    );
    assert_eq!(u128::from(borrow_position.collateral_asset_deposit), 500);
}

#[tokio::test]
async fn fail_liquidation_healthy_borrow() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 500),
    );
    c.borrow(&borrow_user, 300).await;

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    c.liquidate(&liquidator_user, borrow_user.id(), 300).await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_before, collateral_balance_after,
        "Liquidator should not obtain any additional collateral from a rejected liquidation attempt",
    );
    assert_eq!(
        borrow_balance_before, borrow_balance_after,
        "Liquidator should be refunded for a rejected liquidation attempt",
    );

    // ensure borrow position remains unchanged
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(
        u128::from(borrow_position.get_borrow_asset_principal()),
        300,
    );
    assert_eq!(u128::from(borrow_position.collateral_asset_deposit), 500);
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Position is already liquidation locked"]
async fn liquidators_race() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 500),
    );
    c.borrow(&borrow_user, 300).await;
    c.set_collateral_asset_price(0.5).await;

    let balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;

    let (r1, r2) = tokio::join!(
        c.liquidate(&liquidator_user, borrow_user.id(), 300),
        c.liquidate(&liquidator_user, borrow_user.id(), 300),
    );

    let balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        balance_before + 500,
        balance_after,
        "Liquidation should only occur once",
    );

    for o in r1.outcomes() {
        o.clone().into_result().unwrap();
    }

    for o in r2.outcomes() {
        o.clone().into_result().unwrap();
    }
}

#[rstest]
#[tokio::test]
async fn successful_liquidation_only_from_interest() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr = dec!("2");
            c.borrow_mcr_initial = dec!("2");
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );
    c.borrow(&borrow_user, 1_000_000 - 1).await;

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    let timer = Instant::now();
    while timer.elapsed() < Duration::from_secs(5) {
        c.harvest_yield(&supply_user, None, None).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    c.liquidate(&liquidator_user, borrow_user.id(), 2_000_000 * 95 / 100)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        2_000_000,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        2_000_000 * 95 / 100,
        "Liquidation should transfer correct amount of tokens",
    );
}

#[rstest]
#[case((10, 1000), (10, 1000), (9, 1000), (10, 1000), 1710, true)]
#[case((10, 1000), (10, 1000), (9, 1000), (10, 1000), 1700, false)]
#[case((10, -1000), (10, -1000), (9, -1000), (10, -1000), 1710, true)]
#[case((10, -1000), (10, -1000), (9, -1000), (10, -1000), 1700, false)]
#[case((10, 1000), (10, 1000), (10, -1000), (10, 1000), 1, true)]
#[case((10, 1000), (10, 1000), (10, -1000), (10, -1000), 20_0000, false)]
#[case((10, 1000), (10, 1000), (90, 999), (10, 1000), 1710, true)]
#[case((10, 1000), (10, 1000), (90, 999), (10, 1000), 1709, false)]
#[case((10, 1000), (10, 1000), (10, 1000), (11, 1000), 1728, true)]
#[case((10, 1000), (10, 1000), (10, 1000), (11, 1000), 1727, false)]
#[tokio::test]
async fn extreme_prices(
    #[case] (collateral_price, collateral_exponent): (i64, i32),
    #[case] (borrow_price, borrow_exponent): (i64, i32),
    #[case] (new_collateral_price, new_collateral_exponent): (i64, i32),
    #[case] (new_borrow_price, new_borrow_exponent): (i64, i32),
    #[case] liquidate_for: u128,
    #[case] expect_success: bool,
) {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr = dec!("2");
            c.borrow_mcr_initial = dec!("2");
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );

    c.set_collateral_asset_price_exact(pyth::Price {
        price: collateral_price.into(),
        conf: 0.into(),
        expo: collateral_exponent,
        publish_time: 0,
    })
    .await;
    c.set_borrow_asset_price_exact(pyth::Price {
        price: borrow_price.into(),
        conf: 0.into(),
        expo: borrow_exponent,
        publish_time: 0,
    })
    .await;

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1_000_000),
        c.collateralize(&borrow_user, 2000),
    );
    c.borrow(&borrow_user, 1000).await;

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    c.set_collateral_asset_price_exact(pyth::Price {
        price: new_collateral_price.into(),
        conf: 0.into(),
        expo: new_collateral_exponent,
        publish_time: 0,
    })
    .await;
    c.set_borrow_asset_price_exact(pyth::Price {
        price: new_borrow_price.into(),
        conf: 0.into(),
        expo: new_borrow_exponent,
        publish_time: 0,
    })
    .await;
    c.liquidate(&liquidator_user, borrow_user.id(), liquidate_for)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    if expect_success {
        assert_eq!(
            collateral_balance_after - collateral_balance_before,
            2000,
            "Liquidator should obtain all collateral after a successful liquidation",
        );
        assert_eq!(
            borrow_balance_before - borrow_balance_after,
            liquidate_for,
            "Liquidation should transfer correct amount of tokens",
        );
    } else {
        assert_eq!(
            collateral_balance_after - collateral_balance_before,
            0,
            "Liquidator should not obtain collateral",
        );
        assert_eq!(
            borrow_balance_before - borrow_balance_after,
            0,
            "Liquidation should not transfer borrow asset tokens",
        );
    }
}
