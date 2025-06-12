use rstest::rstest;
use test_utils::*;

#[rstest]
#[case([10_000], 10_000)]
#[case([1_000, 9_000], 10_000)]
#[tokio::test]
async fn supply_within_maximum(
    #[case] deposits: impl IntoIterator<Item = u128>,
    #[case] supply_maximum: u128,
) {
    setup_test!(
        extract(c)
        accounts(supply_user)
        config(|c| {
            c.supply_range = (1, Some(supply_maximum)).try_into().unwrap();
        })
    );

    let mut sum = 0;
    for deposit in deposits {
        sum += deposit;
        c.supply(&supply_user, deposit).await;
    }

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
    assert_eq!(
        u128::from(supply_position.get_borrow_asset_deposit_total()),
        sum,
    );
}

#[rstest]
#[case([10_001], 10_000)]
#[case([1, 100_000], 10_000)]
#[case([9_001, 500, 500], 10_000)]
#[case([2], 1)]
#[tokio::test]
#[should_panic = "Smart contract panicked: New supply position is outside of allowable range"]
async fn supply_beyond_maximum(
    #[case] deposits: impl IntoIterator<Item = u128>,
    #[case] supply_maximum: u128,
) {
    setup_test!(
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
