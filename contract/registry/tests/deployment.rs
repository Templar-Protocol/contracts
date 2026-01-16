use near_api_types::AccessKeyPermission;
use near_sandbox::Sandbox;
use near_sdk::serde_json::{self, json};
use tokio::task::JoinSet;

use templar_common::market::YieldWeights;
use test_utils::*;

#[rstest::rstest]
#[tokio::test]
pub async fn deploy_from_registry(#[future(awt)] worker: Sandbox) {
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
        price_oracle.account().id().clone(),
        borrow_asset.account().id().clone(),
        collateral_asset.account().id().clone(),
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
            r.deploy(r.account(), "one", "market@0.0.0", init_args, None)
                .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let init_args = init_args.clone();
        async move {
            r.deploy(r.account(), "two", "market@0.0.0", init_args, None)
                .await
        }
    });

    deployments.spawn({
        let r = r.clone();
        let init_args = init_args.clone();
        async move {
            r.deploy(r.account(), "three", "market@0.0.0", init_args, None)
                .await
        }
    });

    while let Some(market_id) = deployments.join_next().await {
        let market_id = market_id.unwrap();
        let market_account = protocol_account.clone_with_id(market_id.clone());

        let c = UnifiedMarketController::attach(market_account).await;
        assert_eq!(c.configuration, expected_configuration);

        let view_access_keys = c.account.list_access_keys().await;

        assert!(view_access_keys.is_empty());

        eprintln!("Successfully deployed market to {market_id}");
    }
}

#[rstest::rstest]
#[tokio::test]
async fn deploy_with_access_key(#[future(awt)] worker: Sandbox) {
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

    let pk: near_sdk::PublicKey =
        near_api::PublicKey::ED25519(near_api_types::crypto::public_key::ED25519PublicKey(
            ed25519_dalek::SigningKey::from_bytes(&[0x77; 32])
                .verifying_key()
                .to_bytes(),
        ))
        .to_string()
        .parse()
        .unwrap();

    let market_id = r
        .deploy(
            r.account(),
            "market",
            "market@0.0.0".to_string(),
            serde_json::to_vec(&json!({
                "configuration": market_configuration(
                        price_oracle.account().id().clone(),
                        borrow_asset.account().id().clone(),
                        collateral_asset.account().id().clone(),
                        protocol_account.id().clone(),
                        YieldWeights::new_with_supply_weight(1),
                    ),
            }))
            .unwrap(),
            Some(vec![pk.clone()]),
        )
        .await;

    let market = UnifiedMarketController::attach(r.account.clone_with_id(market_id)).await;

    let view_access_keys = market.account().list_access_keys().await;

    assert_eq!(view_access_keys.len(), 1);
    assert_eq!(view_access_keys[0].0.to_string(), String::from(&pk),);
    assert!(matches!(
        view_access_keys[0].1.permission,
        AccessKeyPermission::FullAccess,
    ));
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Market ID collision"]
pub async fn market_id_collision(#[future(awt)] worker: Sandbox) {
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
        price_oracle.account().id().clone(),
        borrow_asset.account().id().clone(),
        collateral_asset.account().id().clone(),
        protocol_account.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let init_args = serde_json::to_vec(&json!({
        "configuration": expected_configuration,
    }))
    .unwrap();

    r.deploy(
        r.account(),
        "market",
        "market@0.0.0",
        init_args.clone(),
        None,
    )
    .await;

    r.deploy(r.account(), "market", "market@0.0.0", init_args, None)
        .await;
}
