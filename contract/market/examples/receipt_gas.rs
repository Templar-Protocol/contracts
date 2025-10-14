#![allow(clippy::wildcard_imports)]

use templar_common::fee::Fee;
use test_utils::*;

#[tokio::main]
async fn main() {
    let worker = worker().await;
    setup_test!(
        worker
        extract(c, insurance_yield_user)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 20_000),
        c.collateralize(&borrow_user, 13_000),
    );

    c.borrow(&borrow_user, 10_000).await;

    // c.repay(&borrow_user, 10_000).await;

    // c.set_collateral_asset_price(0.85).await;
    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        13_000.into(),
        11_000.into(),
    )
    .await;

    // c.liquidate(&liquidator_user, borrow_user.id(), 11_000)
    //     .await;

    // let r = c
    //     .withdraw_static_yield(&insurance_yield_user, None, None)
    //     .await;

    c.create_supply_withdrawal_request(&supply_user, 1_000)
        .await;
    let r = c.execute_next_supply_withdrawal_request(&supply_user).await;

    for receipt in r.receipt_outcomes() {
        eprintln!("{}: {}", receipt.executor_id, receipt.gas_burnt);
    }

    eprintln!("Total gas: {}", r.total_gas_burnt);
}
