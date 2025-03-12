use std::{sync::atomic::Ordering, time::Duration};

use rstest::rstest;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::MS_IN_A_YEAR,
    number::Decimal,
};
use test_utils::*;

#[rstest]
#[case(1_000_000, InterestRateStrategy::linear(dec!("1000000"), dec!("1000000")).unwrap())]
#[case(1_000_000, InterestRateStrategy::linear(dec!("100000"), dec!("5000000")).unwrap())]
#[case(5_000_000,
    InterestRateStrategy::piecewise(Decimal::ZERO, dec!("0.9"), dec!("350"), dec!("6000")).unwrap()
)]
#[case(5_000_000,
    InterestRateStrategy::exponential2(dec!("5"), dec!("800"), dec!("6")).unwrap()
)]
#[tokio::test]
async fn interest_rate(#[case] principal: u128, #[case] strategy: InterestRateStrategy) {
    let SetupEverything {
        c,
        supply_user,
        supply_user_2,
        borrow_user,
        borrow_user_2,
        ..
    } = setup_everything(|c| {
        c.borrow_origination_fee = Fee::zero();
        c.borrow_interest_rate_strategy = strategy.clone();
    })
    .await;

    c.supply(&supply_user, principal * 5).await;
    c.supply(&supply_user_2, principal * 5).await;
    c.collateralize(&borrow_user, principal * 2).await;
    c.collateralize(&borrow_user_2, principal * 2).await;

    let time_outer = std::time::Instant::now();
    tokio::join!(
        c.borrow(&borrow_user, principal),
        c.borrow(&borrow_user_2, principal),
    );
    // wait for ~1 block
    tokio::time::sleep(Duration::from_secs(1)).await;
    let time_inner = std::time::Instant::now();

    let mut iters = 0;

    for _ in 0..3 {
        println!("Sleeping...");
        let done = std::sync::atomic::AtomicBool::new(false);
        tokio::join!(
            async {
                // borrow_user_2 will be continually applying interest while borrow_user_1 does not.
                // They should accumulate (very nearly) the same amount of interest regardless.
                while !done.load(Ordering::Relaxed) {
                    tokio::join!(
                        c.apply_interest(&borrow_user_2),
                        c.harvest_yield(&supply_user_2),
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    iters += 1;
                }
            },
            async {
                tokio::time::sleep(Duration::from_secs(12)).await;
                done.store(true, Ordering::Relaxed);
            }
        );
        println!("Done sleeping!");

        let duration_inner = time_inner.elapsed();
        let (borrow_position_1, borrow_position_2, supply_position_1, supply_position_2) = tokio::join!(
            async { c.get_borrow_position(borrow_user.id()).await.unwrap() },
            async { c.get_borrow_position(borrow_user_2.id()).await.unwrap() },
            async { c.get_supply_position(supply_user.id()).await.unwrap() },
            async { c.get_supply_position(supply_user_2.id()).await.unwrap() },
        );
        let duration_outer = time_outer.elapsed();

        let supply_yield_1 = supply_position_1.borrow_asset_yield.get_total().as_u128()
            + supply_position_1.pending_yield_estimate.as_u128();
        let supply_yield_2 = supply_position_2.borrow_asset_yield.get_total().as_u128()
            + supply_position_2.pending_yield_estimate.as_u128();

        // No yield yet.
        assert_eq!(supply_yield_1, 0);
        assert_eq!(supply_yield_2, 0);

        println!("Borrow position 1: {borrow_position_1:#?}");
        println!("Borrow position 2: {borrow_position_2:#?}");

        let f = principal * strategy.at(dec!("0.2")) / Decimal::from(MS_IN_A_YEAR);

        let approximation_below = (f * duration_inner.as_millis()).to_u128_ceil().unwrap();
        let approximation_above = (f * duration_outer.as_millis()).to_u128_ceil().unwrap();

        let actual_1 = borrow_position_1.borrow_asset_fees.get_total().as_u128()
            + borrow_position_1.pending_fee_estimate.as_u128();
        println!("{approximation_below} <= {actual_1} <= {approximation_above}?");

        assert!(approximation_below <= actual_1);
        assert!(actual_1 <= approximation_above);

        let actual_2 = borrow_position_2.borrow_asset_fees.get_total().as_u128()
            + borrow_position_2.pending_fee_estimate.as_u128();
        println!("{approximation_below} <= {actual_2} <= {approximation_above} + {iters}?");

        assert!(approximation_below <= actual_2);
        assert!(actual_2 <= approximation_above + iters);

        assert!(
            actual_2 >= actual_1,
            "Users should not be able to reduce interest by applying it more frequently"
        );
        assert!(
            actual_2 <= actual_1 + iters,
            "Accuracy should be within # of iters due to rounding up",
        );
    }

    tokio::join!(
        async {
            let borrow_position_before = c.get_borrow_position(borrow_user.id()).await.unwrap();
            c.repay(
                &borrow_user,
                borrow_position_before
                    .get_total_borrow_asset_liability()
                    .as_u128()
                    * 110
                    / 100, /* overpayment */
            )
            .await;
            let borrow_position_after = c.get_borrow_position(borrow_user.id()).await.unwrap();

            assert!(
                borrow_position_after
                    .get_total_borrow_asset_liability()
                    .is_zero(),
                "Borrow should be fully repaid",
            );
        },
        async {
            let borrow_position_before = c.get_borrow_position(borrow_user_2.id()).await.unwrap();
            c.repay(
                &borrow_user_2,
                borrow_position_before
                    .get_total_borrow_asset_liability()
                    .as_u128()
                    * 110
                    / 100, /* overpayment */
            )
            .await;
            let borrow_position_after = c.get_borrow_position(borrow_user_2.id()).await.unwrap();

            assert!(
                borrow_position_after
                    .get_total_borrow_asset_liability()
                    .is_zero(),
                "Borrow should be fully repaid",
            );
        },
    );

    let (supply_position_1, supply_position_2) = tokio::join!(
        async {
            c.harvest_yield(&supply_user).await;
            c.get_supply_position(supply_user.id()).await.unwrap()
        },
        async {
            c.harvest_yield(&supply_user_2).await;
            c.get_supply_position(supply_user_2.id()).await.unwrap()
        },
    );

    assert!(!supply_position_1.borrow_asset_yield.get_total().is_zero());
    assert_eq!(
        supply_position_1.borrow_asset_yield.get_total(),
        supply_position_2.borrow_asset_yield.get_total(),
        "Harvesting yield more often should not change total",
    );
}
