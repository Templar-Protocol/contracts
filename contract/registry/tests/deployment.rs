use near_sdk::serde_json::json;
use templar_common::market::YieldWeights;
use test_utils::{
    accounts,
    controller::{ft::FtController, oracle::OracleController, ContractController},
    market_configuration, setup_registry,
};

#[tokio::test]
pub async fn deploy_from_registry() {
    let worker = near_workspaces::sandbox().await.unwrap();
    let r = setup_registry(&worker).await;

    accounts!(
        worker,
        balance_oracle,
        borrow_asset,
        collateral_asset,
        protocol_account
    );

    let (balance_oracle, borrow_asset, collateral_asset) = tokio::join!(
        OracleController::deploy(balance_oracle),
        FtController::deploy(borrow_asset, "Borrow Asset", "BORROW"),
        FtController::deploy(collateral_asset, "Collateral Asset", "COLLATERAL"),
    );

    let market_id = r
        .deploy_market(
            r.contract().as_account(),
            "market".to_string(),
            json!({
                "configuration": market_configuration(
                    balance_oracle.contract().id().clone(),
                    borrow_asset.contract().id().clone(),
                    collateral_asset.contract().id().clone(),
                    protocol_account.id().clone(),
                    YieldWeights::new_with_supply_weight(1),
                )
            }),
        )
        .await;

    eprintln!("Successfully deployed market to {market_id}");
}
