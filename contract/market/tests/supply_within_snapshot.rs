use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::HarvestYieldMode,
    time_chunk::TimeChunkConfiguration,
};
use test_utils::*;

#[tokio::test]
async fn partial_snapshot_no_earnings() {
    let SetupEverything {
        c,
        borrow_user,
        supply_user,
        supply_user_2,
        ..
    } = setup_everything(|c| {
        c.borrow_origination_fee = Fee::zero();
        c.borrow_interest_rate_strategy =
            InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
        c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
            divisor: (10 * 1000).into(),
        };
    })
    .await;

    println!("Creating first supply position");
    c.supply(&supply_user, 1_000_000).await;

    let snapshot_index_0 = c.get_finalized_snapshots_len().await;
    let mut snapshot_index_1 = snapshot_index_0;

    // Wait for activation of funds
    while snapshot_index_1 != snapshot_index_0 {
        c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default))
            .await;

        snapshot_index_1 = c.get_finalized_snapshots_len().await;
    }

    c.collateralize(&borrow_user, 10_000_000).await;
    c.borrow(&borrow_user, 900_000).await;

    let snapshot_index_1 = c.get_finalized_snapshots_len().await;
    let mut snapshot_index_2 = snapshot_index_1;

    while snapshot_index_2 != snapshot_index_1 {
        tokio::join!(
            c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default)),
            async {
                c.repay(&borrow_user, 1_000_000).await;
                c.borrow(&borrow_user, 900_000).await;
            },
        );

        snapshot_index_2 = c.get_finalized_snapshots_len().await;
    }

    println!("Creating second supply position");
    c.supply(&supply_user_2, 1_000_000).await;

    let (amount_1_start, amount_2_start) = tokio::join!(
        async {
            c.get_supply_position(supply_user.id())
                .await
                .unwrap()
                .borrow_asset_yield
                .get_total()
        },
        async {
            c.get_supply_position(supply_user_2.id())
                .await
                .unwrap()
                .borrow_asset_yield
                .get_total()
        },
    );

    let mut snapshot_index_3 = snapshot_index_2;
    while snapshot_index_3 != snapshot_index_2 {
        tokio::join!(
            c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default)),
            c.harvest_yield(&supply_user_2, Some(HarvestYieldMode::Default)),
            async {
                c.repay(&borrow_user, 1_000_000).await;
                c.borrow(&borrow_user, 900_000).await;
            },
        );

        snapshot_index_3 = c.get_finalized_snapshots_len().await;
    }

    let (amount_1_end, amount_2_end) = tokio::join!(
        async {
            c.get_supply_position(supply_user.id())
                .await
                .unwrap()
                .borrow_asset_yield
                .get_total()
        },
        async {
            c.get_supply_position(supply_user_2.id())
                .await
                .unwrap()
                .borrow_asset_yield
                .get_total()
        },
    );

    println!("1: {amount_1_start} -> {amount_1_end}");
    println!("2: {amount_2_start} -> {amount_2_end}");
}
