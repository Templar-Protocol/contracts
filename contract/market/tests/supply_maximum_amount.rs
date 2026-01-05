use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use templar_common::{market::HarvestYieldMode, time_chunk::TimeChunkConfiguration};
use test_utils::*;

#[rstest]
#[case([10_000], 10_000)]
#[case([1_000, 9_000], 10_000)]
#[case([1; 25], 10_000)]
#[tokio::test]
async fn supply_within_maximum(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] deposits: impl IntoIterator<Item = u128>,
    #[case] supply_maximum: u128,
) {
    setup_test!(
        worker
        extract(c)
        accounts(supply_user)
        config(|c| {
            c.supply_range = (1, Some(supply_maximum)).try_into().unwrap();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1000 * 20);
        })
    );

    let mut sum = 0;
    for deposit in deposits {
        sum += deposit;
        c.supply(&supply_user, deposit).await;
    }

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
    assert_eq!(u128::from(supply_position.get_deposit().total()), sum);
}

#[rstest]
#[case([10_001], 10_000)]
#[case([1, 100_000], 10_000)]
#[case([9_001, 500, 500], 10_000)]
#[case([2], 1)]
#[tokio::test]
#[should_panic = "Smart contract panicked: New supply position is outside of allowable range"]
async fn supply_beyond_maximum(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] deposits: impl IntoIterator<Item = u128>,
    #[case] supply_maximum: u128,
) {
    setup_test!(
        worker
        extract(c)
        accounts(supply_user)
        config(|c| {
            c.supply_range = (1, Some(supply_maximum)).try_into().unwrap();
        })
    );

    for deposit in deposits {
        let r = c.supply(&supply_user, deposit).await;
        for o in r.outcomes() {
            o.clone().into_result().unwrap();
        }
    }
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: New supply position is outside of allowable range"]
async fn harvest_yield_beyond_maximum(#[future(awt)] worker: Worker<Sandbox>) {
    const LIMIT: u128 = 1_000_000;
    setup_test!(
        worker
        extract(c)
        accounts(supply_user, borrow_user)
        config(|c| {
            c.supply_range = (LIMIT, Some(LIMIT)).try_into().unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, LIMIT),
        c.collateralize(&borrow_user, LIMIT * 2),
    );

    c.borrow(&borrow_user, LIMIT * 4 / 5).await;
    c.repay(&borrow_user, None, LIMIT).await;

    c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Compounding))
        .await;
}
