use std::time::Duration;

use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use tokio::time::Instant;

use templar_common::{
    asset::BorrowAssetAmount, dec, fee::Fee, interest_rate_strategy::InterestRateStrategy,
    market::HarvestYieldMode, Decimal, YEAR_PER_MS,
};
use test_utils::*;

#[rstest]
#[case(10_000_000, InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap())]
#[case(10_000_000, InterestRateStrategy::linear(dec!("10"), dec!("500")).unwrap())]
#[case(5_000_000,
    InterestRateStrategy::piecewise(Decimal::ZERO, dec!("0.09"), dec!("35"), dec!("600")).unwrap()
)]
#[case(5_000_000,
    InterestRateStrategy::exponential2(dec!("5"), dec!("800"), dec!("6")).unwrap()
)]
#[tokio::test]
async fn interest_rate(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] principal: u128,
    #[case] strategy: InterestRateStrategy,
) {
    setup_test!(
        worker
        extract(c)
        accounts(
            borrow_user,
            borrow_user_2,
            supply_user,
            supply_user_2
        )
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy = strategy.clone();
        })
    );

    let supply_amount = principal * 5;

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, supply_amount),
        c.supply_and_harvest_until_activation(&supply_user_2, supply_amount),
        c.collateralize(&borrow_user, principal * 5),
        c.collateralize(&borrow_user_2, principal * 5),
    );

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
        eprintln!("Sleeping...");
        let timer = Instant::now();
        // borrow_user_2 will be continually applying interest while borrow_user_1 does not.
        // They should accumulate (very nearly) the same amount of interest regardless.
        while timer.elapsed() < Duration::from_secs(12) {
            tokio::join!(
                c.apply_interest(&borrow_user_2, None, None),
                // Technically, it should be optimal to harvest and compound
                // (withdraw yield and re-deposit) occasionally throughout the
                // duration of the supply.
                c.harvest_yield(&supply_user_2, None, Some(HarvestYieldMode::Default)),
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
            iters += 1;
        }
        eprintln!("Done sleeping!");

        let duration_inner = time_inner.elapsed();
        let (
            borrow_position_1,
            borrow_position_1_pending,
            borrow_position_2,
            borrow_position_2_pending,
            supply_position_1,
            supply_position_1_pending,
            supply_position_2,
            supply_position_2_pending,
            current_snapshot,
        ) = tokio::join!(
            async { c.get_borrow_position(borrow_user.id()).await.unwrap() },
            async {
                c.get_borrow_position_pending_interest(borrow_user.id(), None)
                    .await
                    .unwrap()
            },
            async { c.get_borrow_position(borrow_user_2.id()).await.unwrap() },
            async {
                c.get_borrow_position_pending_interest(borrow_user_2.id(), None)
                    .await
                    .unwrap()
            },
            async { c.get_supply_position(supply_user.id()).await.unwrap() },
            async {
                c.get_supply_position_pending_yield(supply_user.id(), None)
                    .await
                    .unwrap()
            },
            async { c.get_supply_position(supply_user_2.id()).await.unwrap() },
            async {
                c.get_supply_position_pending_yield(supply_user_2.id(), None)
                    .await
                    .unwrap()
            },
            c.get_current_snapshot(),
        );
        let duration_outer = time_outer.elapsed();

        let supply_yield_1 =
            supply_position_1.borrow_asset_yield.get_total() + supply_position_1_pending;
        let supply_yield_2 =
            supply_position_2.borrow_asset_yield.get_total() + supply_position_2_pending;

        let yield_rate = c
            .configuration
            .supply_yield_rate_from_interest(&current_snapshot);

        let supply_yield_min =
            supply_amount * yield_rate * duration_inner.as_millis() * YEAR_PER_MS
                + c.configuration.single_snapshot_maximum_interest();
        let supply_yield_max =
            supply_amount * yield_rate * duration_outer.as_millis() * YEAR_PER_MS
                + c.configuration.single_snapshot_maximum_interest();

        eprintln!("{supply_yield_min} <= {supply_yield_1} <= {supply_yield_max} ?");
        eprintln!("{supply_yield_min} <= {supply_yield_2} <= {supply_yield_max} ?");

        assert!(supply_yield_min <= Decimal::from(supply_yield_1));
        assert!(Decimal::from(supply_yield_1) <= supply_yield_max);
        assert!(supply_yield_min <= Decimal::from(supply_yield_2));
        assert!(Decimal::from(supply_yield_2) <= supply_yield_max);

        eprintln!("Borrow position 1: {borrow_position_1:#?}");
        eprintln!("Borrow position 2: {borrow_position_2:#?}");

        let f = principal * strategy.at(dec!("0.2")) * YEAR_PER_MS;

        let approximation_below = (f * duration_inner.as_millis()).to_u128_floor().unwrap();
        let approximation_above = (f * duration_outer.as_millis()).to_u128_ceil().unwrap();

        let actual_1 = borrow_position_1.interest.get_total() + borrow_position_1_pending;
        eprintln!("{approximation_below} <= {actual_1} <= {approximation_above}?");

        assert!(approximation_below <= actual_1.into());
        assert!(u128::from(actual_1) <= approximation_above);

        let actual_2 = borrow_position_2.interest.get_total() + borrow_position_2_pending;
        eprintln!("{approximation_below} <= {actual_2} <= {approximation_above} + {iters}?");

        assert!(approximation_below <= actual_2.into());
        assert!(u128::from(actual_2) <= approximation_above + iters);

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
            let borrow_position_before_pending = c
                .get_borrow_position_pending_interest(borrow_user.id(), None)
                .await
                .unwrap();
            let r = c
                .repay(
                    &borrow_user,
                    None,
                    u128::from(
                        borrow_position_before.get_total_borrow_asset_liability()
                            + borrow_position_before_pending,
                    ) * 110
                        / 100, /* overpayment */
                )
                .await;
            eprintln!("{r:#?}");
            eprintln!("logs");
            for log in r.logs() {
                eprintln!("\t{log}");
            }
            let borrow_position_after = c.get_borrow_position(borrow_user.id()).await.unwrap();

            assert_eq!(
                borrow_position_after.get_total_borrow_asset_liability(),
                BorrowAssetAmount::zero(),
                "Borrow should be fully repaid",
            );
        },
        async {
            let borrow_position_before = c.get_borrow_position(borrow_user_2.id()).await.unwrap();
            let borrow_position_before_pending = c
                .get_borrow_position_pending_interest(borrow_user_2.id(), None)
                .await
                .unwrap();
            c.repay(
                &borrow_user_2,
                None,
                u128::from(
                    borrow_position_before.get_total_borrow_asset_liability()
                        + borrow_position_before_pending,
                ) * 110
                    / 100, /* overpayment */
            )
            .await;
            let borrow_position_after = c.get_borrow_position(borrow_user_2.id()).await.unwrap();

            assert_eq!(
                borrow_position_after.get_total_borrow_asset_liability(),
                BorrowAssetAmount::zero(),
                "Borrow should be fully repaid",
            );
        },
    );

    let (supply_position_1, supply_position_2) = tokio::join!(
        async {
            c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
                .await;
            c.get_supply_position(supply_user.id()).await.unwrap()
        },
        async {
            c.harvest_yield(&supply_user_2, None, Some(HarvestYieldMode::Default))
                .await;
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
