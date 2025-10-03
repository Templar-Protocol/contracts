use near_sdk::serde_json::{self, json};
use near_workspaces::types::{AccessKeyPermission, SecretKey};
use templar_common::market::YieldWeights;
use test_utils::*;
use tokio::task::JoinSet;

#[tokio::test]
pub async fn deploy_from_registry() {
    let worker = near_workspaces::sandbox_with_version("2.7.0")
        .await
        .unwrap();
    let r = setup_registry(&worker).await;

    accounts!(
        worker,
        price_oracle,
        borrow_asset,
        collateral_asset,
        protocol_account
    );

    let (price_oracle, borrow_asset, collateral_asset) = tokio::join!(
        OracleController::deploy(price_oracle),
        FtController::deploy(borrow_asset, "Borrow Asset", "BORROW"),
        FtController::deploy(collateral_asset, "Collateral Asset", "COLLATERAL"),
    );

    let expected_configuration = market_configuration(
        price_oracle.contract().id().clone(),
        borrow_asset.contract().id().clone(),
        collateral_asset.contract().id().clone(),
        protocol_account.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let init_args = serde_json::to_vec(&json!({
        "configuration": expected_configuration,
    }))
    .unwrap();

    let mut deployments = JoinSet::new();

    deployments.spawn({
        let r = r.clone();
        let init_args = init_args.clone();
        async move {
            r.deploy(
                r.contract().as_account(),
                "one",
                "market@0.0.0",
                init_args,
                None,
            )
            .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let init_args = init_args.clone();
        async move {
            r.deploy(
                r.contract().as_account(),
                "two",
                "market@0.0.0",
                init_args,
                None,
            )
            .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let init_args = init_args.clone();
        async move {
            r.deploy(
                r.contract().as_account(),
                "three",
                "market@0.0.0",
                init_args,
                None,
            )
            .await
        }
    });

    while let Some(market_id) = deployments.join_next().await {
        let market_id = market_id.unwrap();

        let c = UnifiedMarketController::attach(&worker, market_id.clone()).await;
        assert_eq!(c.configuration, expected_configuration);

        let view_access_keys = c.contract().view_access_keys().await.unwrap();
        assert!(view_access_keys.is_empty());

        eprintln!("Successfully deployed market to {market_id}");
    }
}

#[tokio::test]
async fn deploy_with_access_key() {
    let worker = near_workspaces::sandbox_with_version("2.7.0")
        .await
        .unwrap();
    let r = setup_registry(&worker).await;

    accounts!(
        worker,
        price_oracle,
        borrow_asset,
        collateral_asset,
        protocol_account
    );

    let (price_oracle, borrow_asset, collateral_asset) = tokio::join!(
        OracleController::deploy(price_oracle),
        FtController::deploy(borrow_asset, "Borrow Asset", "BORROW"),
        FtController::deploy(collateral_asset, "Collateral Asset", "COLLATERAL"),
    );

    let pk: near_sdk::PublicKey = SecretKey::from_random(near_workspaces::types::KeyType::ED25519)
        .public_key()
        .to_string()
        .parse()
        .unwrap();

    let market_id = r
        .deploy(
            r.contract().as_account(),
            "market",
            "market@0.0.0".to_string(),
            serde_json::to_vec(&json!({
                "configuration": market_configuration(
                        price_oracle.contract().id().clone(),
                        borrow_asset.contract().id().clone(),
                        collateral_asset.contract().id().clone(),
                        protocol_account.id().clone(),
                        YieldWeights::new_with_supply_weight(1),
                    ),
            }))
            .unwrap(),
            Some(vec![pk.clone()]),
        )
        .await;

    let market = UnifiedMarketController::attach(&worker, market_id).await;

    let view_access_keys = market.contract().view_access_keys().await.unwrap();

    assert_eq!(view_access_keys.len(), 1);
    assert_eq!(
        view_access_keys[0].public_key.to_string(),
        String::from(&pk),
    );
    assert!(matches!(
        view_access_keys[0].access_key.permission,
        AccessKeyPermission::FullAccess,
    ));
}

#[tokio::test]
#[should_panic = "Smart contract panicked: Market ID collision"]
pub async fn market_id_collision() {
    let worker = near_workspaces::sandbox_with_version("2.7.0")
        .await
        .unwrap();
    let r = setup_registry(&worker).await;

    accounts!(
        worker,
        price_oracle,
        borrow_asset,
        collateral_asset,
        protocol_account
    );

    let (price_oracle, borrow_asset, collateral_asset) = tokio::join!(
        OracleController::deploy(price_oracle),
        FtController::deploy(borrow_asset, "Borrow Asset", "BORROW"),
        FtController::deploy(collateral_asset, "Collateral Asset", "COLLATERAL"),
    );

    let expected_configuration = market_configuration(
        price_oracle.contract().id().clone(),
        borrow_asset.contract().id().clone(),
        collateral_asset.contract().id().clone(),
        protocol_account.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let init_args = serde_json::to_vec(&json!({
        "configuration": expected_configuration,
    }))
    .unwrap();

    r.deploy(
        r.contract().as_account(),
        "market",
        "market@0.0.0",
        init_args.clone(),
        None,
    )
    .await;

    r.deploy(
        r.contract().as_account(),
        "market",
        "market@0.0.0",
        init_args,
        None,
    )
    .await;
}
