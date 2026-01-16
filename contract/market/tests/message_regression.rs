use near_sandbox::Sandbox;
use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    Gas, NearToken,
};
use rstest::rstest;

use templar_common::interest_rate_strategy::InterestRateStrategy;
use test_utils::{near_api::types::transaction::result::ExecutionResult, *};

#[allow(clippy::needless_pass_by_value)]
fn assert_no_failures<T>(result: ExecutionResult<T>) {
    let failures = result.failures();
    if !failures.is_empty() {
        for failure in failures {
            if let Err(e) = failure.clone().into_result() {
                eprintln!("{e}");
            }
        }
        panic!("Failures detected in execution");
    }
}

#[rstest]
#[tokio::test]
async fn message_regression(#[future(awt)] worker: Sandbox) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user, third_party)
        config(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
        })
    );

    assert_no_failures(
        c.borrow_asset
            .transfer_call(
                &supply_user,
                c.market.account().id(),
                10_000_000,
                r#""Supply""#,
            )
            .await,
    );

    assert_no_failures(
        c.call_exec(
            &supply_user,
            "harvest_yield",
            json!({
                "account_id": supply_user.id(),
                "mode": "Default",
            }),
            NearToken::ZERO,
            Gas::from_tgas(30),
        )
        .await,
    );

    assert_no_failures(
        c.collateral_asset
            .transfer_call(
                &borrow_user,
                c.market.account().id(),
                2_000_000,
                r#""Collateralize""#,
            )
            .await,
    );

    assert_no_failures(
        c.call_exec(
            &borrow_user,
            "borrow",
            json!({
                "amount": U128(1_000_000),
            }),
            NearToken::ZERO,
            Gas::from_tgas(100),
        )
        .await,
    );

    assert_no_failures(
        c.borrow_asset
            .transfer_call(&borrow_user, c.market.account().id(), 250_000, r#""Repay""#)
            .await,
    );

    assert_no_failures(
        c.borrow_asset
            .transfer_call(
                &third_party,
                c.market.account().id(),
                250_000,
                serde_json::to_string(&json!({
                    "RepayAccount": {
                        "account_id": borrow_user.id(),
                    },
                }))
                .unwrap(),
            )
            .await,
    );

    assert_no_failures(
        c.call_exec(
            &borrow_user,
            "withdraw_collateral",
            json!({
                "amount": U128(1_000_000),
            }),
            NearToken::ZERO,
            Gas::from_tgas(100),
        )
        .await,
    );

    assert_no_failures(
        c.call_exec(
            &third_party,
            "apply_interest",
            json!({
                "account_id": borrow_user.id(),
                "snapshot_limit": 100,
            }),
            NearToken::ZERO,
            Gas::from_tgas(100),
        )
        .await,
    );

    c.set_borrow_asset_price(2.0).await;

    assert_no_failures(
        c.borrow_asset
            .transfer_call(
                &third_party,
                c.market.account().id(),
                500_000,
                serde_json::to_string(&json!({
                    "Liquidate": {
                        "account_id": borrow_user.id(),
                        "amount": U128(1_000_000),
                    },
                }))
                .unwrap(),
            )
            .await,
    );
}
