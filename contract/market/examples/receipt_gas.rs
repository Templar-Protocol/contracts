#![allow(clippy::wildcard_imports)]

use templar_common::fee::Fee;
use test_utils::*;

#[tokio::main]
async fn main() {
    setup_test!(
        extract(c, insurance_yield_user)
        accounts(borrow_user, supply_user, liquidator_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
        })
    );

    c.supply(&supply_user, 20_000).await;
    c.collateralize(&borrow_user, 13_000).await;
    c.borrow(&borrow_user, 10_000.into()).await;

    c.set_collateral_asset_price(0.85).await;

    c.liquidate(&liquidator_user, borrow_user.id(), 11_000)
        .await;

    let r = c
        .withdraw_static_yield(&insurance_yield_user, None, None)
        .await;

    for receipt in r.receipt_outcomes() {
        eprintln!("{}: {}", receipt.executor_id, receipt.gas_burnt);
    }
}
