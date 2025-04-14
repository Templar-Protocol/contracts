use near_sdk::serde_json::json;
use templar_common::market::YieldWeights;
use test_utils::*;
use tokio::task::JoinSet;

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

    let expected_configuration = market_configuration(
        balance_oracle.contract().id().clone(),
        borrow_asset.contract().id().clone(),
        collateral_asset.contract().id().clone(),
        protocol_account.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let mut deployments = JoinSet::new();

    deployments.spawn({
        let r = r.clone();
        let expected_configuration = expected_configuration.clone();
        async move {
            r.deploy_market(
                r.contract().as_account(),
                Some("p".to_string()),
                "market@0.0.0".to_string(),
                json!({
                    "configuration": expected_configuration,
                }),
            )
            .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let expected_configuration = expected_configuration.clone();
        async move {
            r.deploy_market(
                r.contract().as_account(),
                Some("p".to_string()),
                "market@0.0.0".to_string(),
                json!({
                    "configuration": expected_configuration,
                }),
            )
            .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let expected_configuration = expected_configuration.clone();
        async move {
            r.deploy_market(
                r.contract().as_account(),
                None,
                "market@0.0.0".to_string(),
                json!({
                    "configuration": expected_configuration,
                }),
            )
            .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let expected_configuration = expected_configuration.clone();
        async move {
            r.deploy_market(
                r.contract().as_account(),
                None,
                "market@0.0.0".to_string(),
                json!({
                    "configuration": expected_configuration,
                }),
            )
            .await
        }
    });

    let market_ids = deployments.join_all().await;

    for id in market_ids {
        let c = UnifiedMarketController::attach(&worker, id.clone()).await;

        assert_eq!(c.configuration, expected_configuration);

        eprintln!("Successfully deployed market to {id}");
    }
}
