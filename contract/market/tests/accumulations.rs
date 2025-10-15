use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::market::HarvestYieldMode;
use test_utils::*;

#[rstest]
#[tokio::test]
async fn third_party_accumulation_executor(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(borrow_user, supply_user, third_party));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, 2000),
    );

    c.borrow(&borrow_user, 1000).await;

    assert!(c
        .apply_interest(&third_party, Some(borrow_user.id()), None)
        .await
        .failures()
        .is_empty());
    assert!(c
        .apply_interest(&borrow_user, Some(borrow_user.id()), None)
        .await
        .failures()
        .is_empty());

    assert!(c.repay(&borrow_user, 1100).await.failures().is_empty());

    c.harvest_yield(
        &supply_user,
        Some(supply_user.id()),
        Some(HarvestYieldMode::Default),
    )
    .await;
    c.harvest_yield(
        &third_party,
        Some(supply_user.id()),
        Some(HarvestYieldMode::Default),
    )
    .await;
    c.harvest_yield(
        &supply_user,
        Some(supply_user.id()),
        Some(HarvestYieldMode::SnapshotLimit(100)),
    )
    .await;
    c.harvest_yield(
        &third_party,
        Some(supply_user.id()),
        Some(HarvestYieldMode::SnapshotLimit(100)),
    )
    .await;
    c.harvest_yield(
        &supply_user,
        Some(supply_user.id()),
        Some(HarvestYieldMode::Compounding),
    )
    .await;
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Only the position holder can compound yield"]
async fn third_party_cannot_compound_yield(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(borrow_user, supply_user, third_party));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, 2000),
    );

    c.borrow(&borrow_user, 1000).await;
    c.repay(&borrow_user, 1100).await;

    c.harvest_yield(
        &third_party,
        Some(supply_user.id()),
        Some(HarvestYieldMode::Compounding),
    )
    .await;
}
