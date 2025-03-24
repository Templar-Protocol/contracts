use rstest::rstest;
use templar_common::asset::FungibleAssetAmount;
use test_utils::*;

#[rstest]
#[case([10_000], 10_000)]
#[case([1_000, 9_000], 10_000)]
#[tokio::test]
async fn supply_within_maximum(
    #[case] deposits: impl IntoIterator<Item = u128>,
    #[case] supply_maximum: u128,
) {
    let SetupEverything { c, supply_user, .. } = setup_everything(|c| {
        c.supply_maximum_amount = Some(FungibleAssetAmount::new(supply_maximum));
    })
    .await;

    let mut sum = 0;
    for deposit in deposits {
        sum += deposit;
        c.supply(&supply_user, deposit).await;
    }

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
    assert_eq!(u128::from(supply_position.get_borrow_asset_deposit()), sum);
}

#[rstest]
#[case([10_001], 10_000)]
#[case([1, 100_000], 10_000)]
#[case([9_001, 500, 500], 10_000)]
#[case([1], 0)]
#[tokio::test]
#[should_panic = "Smart contract panicked: New supply position cannot exceed configured supply maximum"]
async fn supply_beyond_maximum(
    #[case] deposits: impl IntoIterator<Item = u128>,
    #[case] supply_maximum: u128,
) {
    let SetupEverything { c, supply_user, .. } = setup_everything(|c| {
        c.supply_maximum_amount = Some(FungibleAssetAmount::new(supply_maximum));
    })
    .await;

    for deposit in deposits {
        let r = c.supply(&supply_user, deposit).await;
        for o in r.outcomes() {
            o.clone().into_result().unwrap();
        }
    }
}
