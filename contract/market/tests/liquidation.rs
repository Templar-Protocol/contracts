use std::time::Duration;

use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use templar_common::{
    dec,
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::HarvestYieldMode,
    oracle::pyth,
    price::{Appraise, Convert},
    Decimal,
};
use test_utils::*;
use tokio::time::Instant;

#[rstest]
#[tokio::test]
async fn successful_liquidation_totally_underwater(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
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
    let (collateral, price) = c.liquidatable_collateral_fmv(borrow_user.id()).await;
    assert_eq!(
        collateral,
        500.into(),
        "All collateral should be liquidatable",
    );
    c.liquidate(&liquidator_user, borrow_user.id(), collateral, price)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        collateral.into(),
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        price.into(),
        "Liquidation should transfer correct amount of tokens",
    );
}

#[rstest]
#[tokio::test]
async fn successful_liquidation_exactly_to_zero(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
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

    // Set price to liquidate 100% of both collateral and liability.
    c.set_collateral_asset_price(2_f64 / 3_f64).await;
    let (collateral, price) = c.liquidatable_collateral_fmv(borrow_user.id()).await;
    assert_eq!(
        collateral,
        500.into(),
        "All collateral should be liquidatable",
    );

    let storage_before = c
        .storage_balance_of(borrow_user.id().clone())
        .await
        .unwrap();
    eprintln!("Storage before: {storage_before:?}");

    c.liquidate(&liquidator_user, borrow_user.id(), collateral, price)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        collateral.into(),
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        price.into(),
        "Liquidation should transfer correct amount of tokens",
    );

    // Should clean up borrow position when liquidation brings both liability and collateral down to zero.
    let position = c.get_borrow_position(borrow_user.id()).await;
    assert_eq!(position, None);

    let storage_after = c
        .storage_balance_of(borrow_user.id().clone())
        .await
        .unwrap();

    eprintln!("Storage after: {storage_after:?}");

    assert!(storage_after.available > storage_before.available);
}

// Caveat to this test: Make sure that the yield distribution value is
// divisible by 10 for easy maths.
#[rstest]
#[case(110, 5000, 2450, 50, dec!("1"))]
#[case(120, 1250, 1000, 88, dec!("1"))]
#[case(120, 1250, 1000, 88, dec!(".973"))]
#[case(120, 1250, 1000, 88, dec!(".95"))]
#[tokio::test]
async fn successful_liquidation_good_debt_under_mcr(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] mcr: u16,
    #[case] collateral_amount: u128,
    #[case] borrow_amount: u128,
    #[case] collateral_asset_price_pct: u128,
    #[case] fmv_frac: Decimal,
) {
    setup_test!(
        worker
        extract(c, protocol_yield_user, insurance_yield_user)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_origination_fee = Fee::Flat(10.into());
            c.borrow_mcr_liquidation = Decimal::from(mcr) / 100u32;
            c.borrow_mcr_maintenance = Decimal::from(mcr) / 100u32;
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, collateral_amount),
    );
    c.borrow(&borrow_user, borrow_amount).await;

    let position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(position.fees, 10.into());

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    c.set_collateral_asset_price(
        (Decimal::from(collateral_asset_price_pct) / 100u32).to_f64_lossy(),
    )
    .await;
    let (liquidate, price) = c.liquidatable_collateral_fmv(borrow_user.id()).await;
    eprintln!("Liquidating {liquidate} of {collateral_amount}");
    let price = (u128::from(price) * fmv_frac)
        .to_u128_ceil()
        .unwrap()
        .into();
    c.liquidate(&liquidator_user, borrow_user.id(), liquidate, price)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        liquidate.into(),
        "Liquidator should obtain collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        price.into(),
        "Liquidation should transfer correct amount of tokens",
    );

    let yield_amount: u128 = price.saturating_sub(borrow_amount).max(10.into()).into();

    // finalize a snapshot
    c.apply_interest(&borrow_user, None, None).await;

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
            c.accumulate_static_yield(&protocol_yield_user, None, None)
                .await;
            let protocol_yield = c.get_static_yield(protocol_yield_user.id()).await.unwrap();
            assert_eq!(u128::from(protocol_yield.get_total()), yield_amount / 10);
        },
        async {
            c.accumulate_static_yield(&insurance_yield_user, None, None)
                .await;
            let insurance_yield = c.get_static_yield(insurance_yield_user.id()).await.unwrap();
            assert_eq!(u128::from(insurance_yield.get_total()), yield_amount / 10);
        },
        async {
            let prices = c.get_prices().await;
            let status = c.get_borrow_status(borrow_user.id(), prices).await;
            if u128::from(liquidate) == collateral_amount {
                // 100% liquidated -> position deleted
                assert_eq!(status, None);
            } else {
                assert!(status.unwrap().is_healthy());
            }
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
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] mcr: u16,
    #[case] maximum_spread_pct: u16,
    #[case] spread_pct: u16,
) {
    assert!(spread_pct <= maximum_spread_pct);

    let liquidation_maximum_spread: Decimal = Decimal::from(maximum_spread_pct) / 100u32;
    let target_spread: Decimal = Decimal::from(spread_pct) / 100u32;
    let mcr: Decimal = Decimal::from(mcr) / 100u32;

    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr_liquidation = mcr;
            c.borrow_mcr_maintenance = mcr;
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

    c.set_collateral_asset_price(collateral_asset_price.to_f64_lossy())
        .await;
    let (collateral, price) = c.liquidatable_collateral_fmv(borrow_user.id()).await;
    let price = (u128::from(price) * (Decimal::ONE - target_spread))
        .to_u128_ceil()
        .unwrap()
        .into();
    c.liquidate(&liquidator_user, borrow_user.id(), collateral, price)
        .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        collateral.into(),
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        price.into(),
        "Liquidation should transfer correct amount of tokens",
    );
}

#[rstest]
#[tokio::test]
async fn fail_liquidation_too_little_attached(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
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
    c.liquidate(&liquidator_user, borrow_user.id(), 500.into(), 150.into())
        .await;

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

    let prices = c.get_prices().await;
    let status = c.get_borrow_status(borrow_user.id(), prices).await.unwrap();
    assert!(status.is_liquidation());
}

#[rstest]
#[tokio::test]
async fn fail_liquidation_healthy_borrow(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
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

    c.liquidate(&liquidator_user, borrow_user.id(), 500.into(), 300.into())
        .await;

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

    let prices = c.get_prices().await;
    let status = c.get_borrow_status(borrow_user.id(), prices).await.unwrap();
    assert!(status.is_healthy());
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Attempt to liquidate more collateral than is currently eligible"]
async fn liquidators_race(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
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

    let (collateral, price) = c
        .liquidatable_collateral_with_spread(borrow_user.id())
        .await;

    let (r1, r2) = tokio::join!(
        c.liquidate(&liquidator_user, borrow_user.id(), collateral, price),
        c.liquidate(&liquidator_user, borrow_user.id(), collateral, price),
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
async fn successful_liquidation_only_from_interest(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr_liquidation = dec!("1.9997");
            c.borrow_mcr_maintenance = dec!("2");
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
        c.apply_interest(&borrow_user, None, None).await;
        let position = c.get_borrow_position(borrow_user.id()).await.unwrap();
        eprintln!("Liability: {}", position.get_total_borrow_asset_liability());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let (collateral, price) = c
        .liquidatable_collateral_with_spread(borrow_user.id())
        .await;

    assert!(!collateral.is_zero());
    assert!(!price.is_zero());

    let r = c
        .liquidate(&liquidator_user, borrow_user.id(), collateral, price)
        .await;

    for o in r.outcomes() {
        o.clone().into_result().unwrap();
    }

    let prices = c.get_prices().await;
    let status = c.get_borrow_status(borrow_user.id(), prices).await.unwrap();

    assert!(
        !status.is_liquidation(),
        "Borrow should be healthy after liquidation of all liquidatable collateral",
    );

    let position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    eprintln!(
        "Collateral after liquidate: {}",
        position.get_total_collateral_amount()
    );
    eprintln!(
        "Liability after liquidate: {}",
        position.get_total_borrow_asset_liability()
    );

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        collateral.into(),
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        price.into(),
        "Liquidation should transfer correct amount of tokens",
    );
}

#[rstest]
#[case((10, 1000), (10, 1000), (9, 1000), (10, 1000))]
#[case((10, -1000), (10, -1000), (9, -1000), (10, -1000))]
#[case((10, 1000), (10, 1000), (90, 999), (10, 1000))]
#[case((10, 1000), (10, 1000), (10, 1000), (11, 1000))]
#[case((10, 1000), (10, 1000), (10, -1000), (10, 1000))]
#[tokio::test]
async fn extreme_prices(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] (collateral_price, collateral_exponent): (i64, i32),
    #[case] (borrow_price, borrow_exponent): (i64, i32),
    #[case] (new_collateral_price, new_collateral_exponent): (i64, i32),
    #[case] (new_borrow_price, new_borrow_exponent): (i64, i32),
) {
    use templar_common::oracle::pyth::PythTimestamp;

    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
    );

    c.set_collateral_asset_price_exact(Some(pyth::Price {
        price: collateral_price.into(),
        conf: 0.into(),
        expo: collateral_exponent,
        publish_time: PythTimestamp::from_secs(0),
    }))
    .await;
    c.set_borrow_asset_price_exact(Some(pyth::Price {
        price: borrow_price.into(),
        conf: 0.into(),
        expo: borrow_exponent,
        publish_time: PythTimestamp::from_secs(0),
    }))
    .await;

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1_000_000),
        c.collateralize(&borrow_user, 2000),
    );
    c.borrow(&borrow_user, 1000).await;

    let borrow_position_before = c.get_borrow_position(borrow_user.id()).await.unwrap();

    let collateral_balance_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    tokio::join!(
        c.set_collateral_asset_price_exact(Some(pyth::Price {
            price: new_collateral_price.into(),
            conf: 0.into(),
            expo: new_collateral_exponent,
            publish_time: PythTimestamp::from_secs(0),
        })),
        c.set_borrow_asset_price_exact(Some(pyth::Price {
            price: new_borrow_price.into(),
            conf: 0.into(),
            expo: new_borrow_exponent,
            publish_time: PythTimestamp::from_secs(0),
        })),
    );
    let (liquidate, price) = c
        .liquidatable_collateral_with_spread(borrow_user.id())
        .await;

    eprintln!("Collateral: {liquidate:?}");
    eprintln!("Price: {price:?}");
    assert!(!liquidate.is_zero());
    assert!(!price.is_zero());

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        liquidate,
        price - 1, // offer too low at first
    )
    .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_before, collateral_balance_after,
        "Liquidator should not obtain collateral",
    );
    assert_eq!(
        borrow_balance_before, borrow_balance_after,
        "Liquidation should not transfer borrow asset tokens",
    );

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        liquidate,
        price, // offer enough this time
    )
    .await;

    let collateral_balance_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        liquidate.into(),
        "Liquidator should obtain collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        price.into(),
        "Liquidation should transfer correct amount of tokens",
    );

    let borrow_position_after = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(
        borrow_position_before.get_total_collateral_amount()
            - borrow_position_after.get_total_collateral_amount(),
        liquidate,
        "Position collateral should decrease by the amount purchased by the liquidator"
    );
    assert_eq!(
        borrow_position_before.get_total_borrow_asset_liability()
            - borrow_position_after.get_total_borrow_asset_liability(),
        price,
        "Position liability should decrease by the amount paid by the liquidator, sans fees"
    );
}

#[rstest]
#[tokio::test]
async fn partial_liquidation(#[future(awt)] worker: Worker<Sandbox>) {
    let spread = dec!("0.05");
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, liquidator_alice, liquidator_bob)
        config(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.liquidation_maximum_spread = spread;
        })
    );

    let compensate_initial_fee = (100_000u128 * c.configuration.single_snapshot_maximum_interest())
        .to_u128_ceil()
        .unwrap()
        * 2;

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1_000_000),
        c.collateralize(&borrow_user, 200_000 + compensate_initial_fee),
    );
    c.borrow(&borrow_user, 100_000).await;

    c.set_borrow_asset_price(1.5f64).await;

    let price_pair = c
        .configuration
        .price_oracle_configuration
        .create_price_pair(&c.get_prices().await)
        .unwrap();
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    let liquidate_collateral = borrow_position.liquidatable_collateral(
        &price_pair,
        c.configuration.borrow_mcr_maintenance,
        c.configuration.liquidation_maximum_spread,
    );
    let pay_for_collateral = price_pair
        .convert(liquidate_collateral)
        .to_u128_ceil()
        .unwrap()
        .into();

    eprintln!("Pay for collateral: {pay_for_collateral}");
    eprintln!("Collateral to liquidate: {liquidate_collateral}");

    let liability = borrow_position.get_total_borrow_asset_liability() - pay_for_collateral;
    let collateral = borrow_position.get_total_collateral_amount() - liquidate_collateral;
    let new_cr = price_pair
        .valuation(collateral)
        .ratio(price_pair.valuation(liability))
        .unwrap();

    eprintln!("New CR: {new_cr}");

    assert!(
        new_cr >= c.configuration.borrow_mcr_liquidation,
        "New position should not be in liquidation anymore",
    );

    let collateral_before_alice = c.collateral_asset.balance_of(liquidator_alice.id()).await;
    let borrow_before_alice = c.borrow_asset.balance_of(liquidator_alice.id()).await;
    let collateral_before_bob = c.collateral_asset.balance_of(liquidator_bob.id()).await;
    let borrow_before_bob = c.borrow_asset.balance_of(liquidator_bob.id()).await;

    // First liquidation
    c.liquidate(
        &liquidator_alice,
        borrow_user.id(),
        liquidate_collateral,
        pay_for_collateral,
    )
    .await;
    // Second liquidation
    c.liquidate(
        &liquidator_bob,
        borrow_user.id(),
        liquidate_collateral,
        pay_for_collateral,
    )
    .await;

    let collateral_after_alice = c.collateral_asset.balance_of(liquidator_alice.id()).await;
    let borrow_after_alice = c.borrow_asset.balance_of(liquidator_alice.id()).await;
    let collateral_after_bob = c.collateral_asset.balance_of(liquidator_bob.id()).await;
    let borrow_after_bob = c.borrow_asset.balance_of(liquidator_bob.id()).await;

    assert_eq!(
        collateral_after_alice - collateral_before_alice,
        liquidate_collateral.into(),
        "Alice receives collateral",
    );
    assert_eq!(
        collateral_before_bob, collateral_after_bob,
        "Bob does not receive collateral",
    );
    assert_eq!(
        borrow_before_alice - borrow_after_alice,
        pay_for_collateral.into(),
        "Alice pays for collateral",
    );
    assert_eq!(
        borrow_before_bob, borrow_after_bob,
        "Bob does not pay for for collateral",
    );

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    let price_pair = c
        .configuration
        .price_oracle_configuration
        .create_price_pair(&c.get_prices().await)
        .unwrap();
    let cr = borrow_position
        .collateralization_ratio(&price_pair)
        .unwrap();
    eprintln!("CR: {cr}");
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Liquidation offer too low"]
async fn partial_liquidation_fail_offer_too_little(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
        })
    );

    c.set_collateral_asset_price(5f64).await;

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1_000_000),
        c.collateralize(&borrow_user, 150_000),
    );
    c.borrow(&borrow_user, 100_000).await;

    c.set_collateral_asset_price(1f64).await;

    let collateral_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    let r = c
        .liquidate(
            &liquidator_user,
            borrow_user.id(),
            50_000.into(),
            10_000.into(),
        )
        .await;

    let collateral_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_before, collateral_after,
        "Liquidator should not receive any collateral asset",
    );
    assert_eq!(
        borrow_before, borrow_after,
        "Liquidator should not send any borrow asset",
    );

    let prices = c.get_prices().await;
    let status = c.get_borrow_status(borrow_user.id(), prices).await.unwrap();
    assert!(status.is_liquidation());

    for outcome in r.outcomes() {
        outcome.clone().into_result().unwrap();
    }
}

#[rstest]
#[case(&[dec!("0.5"), dec!("0.49")])]
#[case(&[dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.096")])]
#[case(&[dec!("0.5"), dec!("0.25"), dec!("0.125"), dec!("0.0625"), dec!("0.06235")])]
#[tokio::test]
async fn many_little_partial_liquidations(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] pattern: &[Decimal],
) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
        })
    );

    c.set_collateral_asset_price(5f64).await;

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1_000_000),
        c.collateralize(&borrow_user, 150_000),
    );
    c.borrow(&borrow_user, 100_000).await;

    c.set_collateral_asset_price(1f64).await;

    let collateral_before = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_before = c.borrow_asset.balance_of(liquidator_user.id()).await;

    let (collateral, price) = c
        .liquidatable_collateral_with_spread(borrow_user.id())
        .await;
    let collateral = u128::from(collateral);
    let price = u128::from(price);

    let mut total_collateral = 0;
    let mut total_paid = 0;

    for fraction in pattern {
        let collateral_fraction = (collateral * fraction).to_u128_floor().unwrap();
        let price_fraction = (price * fraction).to_u128_ceil().unwrap();
        eprintln!("Collateral fraction: {collateral_fraction}");
        eprintln!("Price fraction: {price_fraction}");
        let r = c
            .liquidate(
                &liquidator_user,
                borrow_user.id(),
                collateral_fraction.into(),
                price_fraction.into(),
            )
            .await;
        for outcome in r.outcomes() {
            outcome.clone().into_result().unwrap();
        }
        total_collateral += collateral_fraction;
        total_paid += price_fraction;
        eprintln!("Running total collateral obtained: {total_collateral}");
        eprintln!("Running total borrow paid: {total_paid}");
    }

    let collateral_after = c.collateral_asset.balance_of(liquidator_user.id()).await;
    let borrow_after = c.borrow_asset.balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_after - collateral_before,
        total_collateral,
        "Liquidator should receive the requested amount of collateral asset",
    );
    assert_eq!(
        borrow_before - borrow_after,
        total_paid,
        "Liquidator should pay the correct amount of borrow asset",
    );
}
