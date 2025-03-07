use std::{sync::atomic::Ordering, time::Duration};

use rstest::rstest;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::MS_IN_A_YEAR,
    number::Decimal,
};
use test_utils::*;

#[test]
fn test_strategy() {
    let s = InterestRateStrategy::linear(dec!("1000000"), dec!("1000000")).unwrap();
    let duration_ms = Decimal::from(30 * 1000u128);
    let ms_in_a_year = dec!("31556952000");
    let principal = 1_000_000u128;
    let usage_ratio = dec!("0.2");
    println!(
        "{}",
        s.at(usage_ratio) * duration_ms / ms_in_a_year * principal
    );
    // 30s -> 950662.1552043429
}

#[rstest]
#[case(1_000_000, InterestRateStrategy::linear(dec!("1000000"), dec!("1000000")).unwrap())]
#[case(1_000_000, InterestRateStrategy::linear(dec!("100000"), dec!("5000000")).unwrap())]
#[tokio::test]
async fn interest_rate(#[case] principal: u128, #[case] strategy: InterestRateStrategy) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        borrow_user_2,
        ..
    } = setup_everything(|c| {
        c.borrow_origination_fee = Fee::zero();
        c.borrow_interest_rate_strategy = strategy.clone();
    })
    .await;

    c.supply(&supply_user, principal * 10).await;
    c.collateralize(&borrow_user, principal * 2).await;
    c.collateralize(&borrow_user_2, principal * 2).await;

    let time_outer = std::time::Instant::now();
    tokio::join!(
        c.borrow(&borrow_user, principal, EQUAL_PRICE),
        c.borrow(&borrow_user_2, principal, EQUAL_PRICE),
    );
    // wait for ~1 block
    tokio::time::sleep(Duration::from_secs(1)).await;
    let time_inner = std::time::Instant::now();

    for _ in 0..3 {
        println!("Sleeping...");
        let done = std::sync::atomic::AtomicBool::new(false);
        tokio::join!(
            async {
                // borrow_user_2 will be continually applying interest while borrow_user_1 does not.
                // They should accumulate the same amount of interest regardless.
                while !done.load(Ordering::Relaxed) {
                    c.apply_interest(&borrow_user_2).await;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            },
            async {
                tokio::time::sleep(Duration::from_secs(12)).await;
                done.store(true, Ordering::Relaxed);
            }
        );
        println!("Done sleeping!");

        let duration_inner = time_inner.elapsed();
        let (borrow_position_1, borrow_position_2) = tokio::join!(
            async { c.get_borrow_position(borrow_user.id()).await.unwrap() },
            async { c.get_borrow_position(borrow_user_2.id()).await.unwrap() },
        );
        let duration_outer = time_outer.elapsed();

        println!(
            "Borrow position 1 fees: {:#?}",
            borrow_position_1.borrow_asset_fees,
        );
        println!(
            "Borrow position 2 fees: {:#?}",
            borrow_position_2.borrow_asset_fees,
        );

        let f = principal * strategy.at(dec!("0.2")) / Decimal::from(MS_IN_A_YEAR);

        let approximation_below = (f * duration_inner.as_millis()).to_u128_ceil().unwrap();
        let approximation_above = (f * duration_outer.as_millis()).to_u128_ceil().unwrap();

        let actual_1 = borrow_position_1.borrow_asset_fees.get_total().as_u128();
        println!("{approximation_below} <= {actual_1} <= {approximation_above}?");

        let actual_2 = borrow_position_2.borrow_asset_fees.get_total().as_u128();
        println!("{approximation_below} <= {actual_2} <= {approximation_above}?");

        assert!(approximation_below <= actual_1);
        assert!(actual_1 <= approximation_above);

        assert!(approximation_below <= actual_2);
        assert!(actual_2 <= approximation_above);

        assert!(
            actual_2 >= actual_1,
            "Users should not be able to reduce interest by applying it more frequently"
        );
        assert!(
            actual_1 / (actual_2 - actual_1) >= 50_000,
            "Accounting accuracy is within 0.002%"
        );
    }
}
