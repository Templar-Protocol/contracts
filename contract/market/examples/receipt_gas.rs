use templar_common::fee::Fee;
use test_utils::{setup_everything, SetupEverything};

#[tokio::main]
async fn main() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        liquidator_user,
        insurance_yield_user,
        ..
    } = setup_everything(|c| {
        c.borrow_origination_fee = Fee::zero();
    })
    .await;

    c.supply(&supply_user, 20_000).await;
    c.collateralize(&borrow_user, 13_000).await;
    c.borrow(&borrow_user, 10_000).await;

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
