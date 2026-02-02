use near_sdk::AccountIdRef;
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use templar_common::interest_rate_strategy::InterestRateStrategy;
use test_utils::*;

#[derive(Debug)]
enum RepayAccount {
    Implicit,
    SpecifySelf,
    SpecifyOther,
}

#[rstest]
#[tokio::test]
async fn repay(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(1, 999_999, 1_000_000, 1_000_001, 2_000_000)] repay_amount: u128,
    #[values(
        RepayAccount::Implicit,
        RepayAccount::SpecifySelf,
        RepayAccount::SpecifyOther
    )]
    account: RepayAccount,
) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, third_party)
        config(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );

    c.borrow(&borrow_user, 1_000_000).await;

    let payer = match account {
        RepayAccount::SpecifyOther => &third_party,
        _ => &borrow_user,
    };

    let account_id_option: Option<&AccountIdRef> = match account {
        RepayAccount::Implicit => None,
        _ => Some(borrow_user.id()),
    };

    let balance_before = c.borrow_asset.balance_of(payer.id()).await;
    let position_before = c.get_borrow_position(borrow_user.id()).await.unwrap();

    let liability = position_before.get_total_borrow_asset_liability();

    c.repay(payer, account_id_option, repay_amount).await;

    let balance_after = c.borrow_asset.balance_of(payer.id()).await;
    let position_after = c.get_borrow_position(borrow_user.id()).await.unwrap();

    if repay_amount <= u128::from(liability) {
        assert_eq!(balance_after, balance_before - repay_amount);
        assert_eq!(
            position_after.get_total_borrow_asset_liability(),
            liability - repay_amount,
        );
    } else {
        assert_eq!(balance_after, balance_before - u128::from(liability));
        assert_eq!(position_after.get_total_borrow_asset_liability(), 0.into());
    }
}
