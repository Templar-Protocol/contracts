use std::{sync::atomic::Ordering, time::Duration};

use rstest::rstest;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::HarvestYieldMode,
};
use test_utils::*;

#[rstest]
#[case(1_000_000, InterestRateStrategy::linear(dec!("1000000"), dec!("1000000")).unwrap(), HarvestYieldMode::Compounding)]
#[case(1_000_000, InterestRateStrategy::linear(dec!("1000000"), dec!("1000000")).unwrap(), HarvestYieldMode::Default)]
#[tokio::test]
async fn compounding_yield(
    #[case] principal: u128,
    #[case] strategy: InterestRateStrategy,
    #[case] compounding: HarvestYieldMode,
) {
    let SetupEverything {
        c,
        supply_user,
        supply_user_2,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.borrow_origination_fee = Fee::zero();
        c.borrow_interest_rate_strategy = strategy.clone();
    })
    .await;

    c.supply(&supply_user, principal * 5).await;
    c.supply(&supply_user_2, principal * 5).await;
    c.collateralize(&borrow_user, principal * 2).await;

    c.borrow(&borrow_user, principal.into()).await;

    eprintln!("Sleeping...");
    let mut iters = 0;
    let done = std::sync::atomic::AtomicBool::new(false);
    tokio::join!(
        async {
            while !done.load(Ordering::Relaxed) {
                c.harvest_yield(&supply_user_2, Some(compounding)).await;
                iters += 1;
            }
        },
        async {
            while !done.load(Ordering::Relaxed) {
                let position = c.get_borrow_position(borrow_user.id()).await.unwrap();
                c.repay(
                    &borrow_user,
                    u128::from(position.get_total_borrow_asset_liability()) * 120 / 100,
                )
                .await;
                c.borrow(&borrow_user, principal.into()).await;
            }
        },
        async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            done.store(true, Ordering::Relaxed);
        }
    );
    eprintln!("Done sleeping!");

    c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default))
        .await;

    let (supply_position_1_after, supply_position_2_after) = tokio::join!(
        async { c.get_supply_position(supply_user.id()).await.unwrap() },
        async { c.get_supply_position(supply_user_2.id()).await.unwrap() },
    );

    let supply_yield_1 = u128::from(supply_position_1_after.get_borrow_asset_deposit())
        + u128::from(supply_position_1_after.borrow_asset_yield.get_total())
        + u128::from(supply_position_1_after.borrow_asset_yield.pending_estimate)
        - principal * 5;
    let supply_yield_2 = u128::from(supply_position_2_after.get_borrow_asset_deposit())
        + u128::from(supply_position_2_after.borrow_asset_yield.get_total())
        + u128::from(supply_position_2_after.borrow_asset_yield.pending_estimate)
        - principal * 5;

    eprintln!("supply 1 yield: {supply_yield_1:#?}");
    eprintln!("supply 2 yield: {supply_yield_2:#?}");
    eprintln!("iterations: {iters}");

    if matches!(compounding, HarvestYieldMode::Compounding) {
        // Supply user 2 will be rounded DOWN each iteration.
        // Ensure that it is compounding, so each iteration should add (much) more
        // than 1.
        assert!(supply_yield_2 > supply_yield_1 + iters);
    } else {
        assert!(supply_yield_1 >= supply_yield_2);
        assert!(supply_yield_1 < supply_yield_2 + iters + 1);
    }
}
