mod common;

use common::{setup_ctx, signer_args};
use near_sdk::{serde_json::json, AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    registry::DeployMode,
};
use templar_manager::commands::{
    market::{create::CreateMarket, deploy::DeployMarket, remove::MarketRemove},
    registry::{
        deploy::DeployRegistry,
        version::add::{AddVersion, Package},
    },
    DeployFromRegistry, FixedContractWasm, SignerArgs,
};
use test_utils::{accounts, market_configuration, worker};

#[rstest]
#[tokio::test]
async fn market_deploy(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, market_account, oracle, borrow, collateral, protocol);

    let config = market_configuration(
        oracle.id().clone(),
        borrow.id().clone(),
        collateral.id().clone(),
        protocol.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let init_args = json!({ "configuration": config });

    DeployMarket {
        signer: signer_args(&market_account),
        contract_wasm: FixedContractWasm { no_build: true },
        init_args: serde_json::to_string(&init_args).unwrap(),
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify the contract is deployed by querying its configuration.
    let stored_config: MarketConfiguration = ctx
        .near
        .view(market_account.id(), "get_configuration")
        .await
        .unwrap()
        .json()
        .unwrap();

    assert_eq!(stored_config, config);
}

#[rstest]
#[tokio::test]
async fn market_create_from_registry_and_removal(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(true, false)] force: bool,
) {
    let ctx = setup_ctx(&worker);

    accounts!(worker, registry, oracle, borrow, collateral, protocol);

    let registry_signer = signer_args(&registry);

    DeployRegistry {
        signer: registry_signer.clone(),
        contract: FixedContractWasm { no_build: true },
        no_init: false,
    }
    .run(&ctx)
    .await
    .unwrap();

    AddVersion {
        signer: registry_signer.clone(),
        contract_wasm: FixedContractWasm { no_build: true },
        package: Package {
            market: true,
            uac: false,
            proxy_oracle: false,
            redstone_adapter: false,
            package: None,
        },
        registry_id: registry.id().clone(),
        version_key: Some("market@test".to_string()),
        deploy_mode: DeployMode::Normal,
        deposit: None,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Create market from registry. The registry's deploy method is owner-only,
    // so we must sign with the registry account.

    let config = market_configuration(
        oracle.id().clone(),
        borrow.id().clone(),
        collateral.id().clone(),
        protocol.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    CreateMarket {
        signer: registry_signer.clone(),
        deploy: DeployFromRegistry {
            registry_id: registry.id().clone(),
            version_key: "market@test".to_string(),
            name: "mkt".to_string(),
            with_full_access_key: vec![],
            no_signer_full_access_key: false,
            deposit: Some(NearToken::from_near(6)),
        },
        configuration: serde_json::to_string(&config).unwrap(),
    }
    .run(&ctx)
    .await
    .unwrap();

    let market_id: AccountId = format!("mkt.{}", registry.id()).parse().unwrap();

    // Verify market exists by querying configuration
    let stored_config: MarketConfiguration = ctx
        .near
        .view(&market_id, "get_configuration")
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(stored_config, config);

    // Now we remove the market
    MarketRemove {
        signer: SignerArgs {
            account_id: market_id.clone(),
            secret_key: registry.secret_key().to_string().parse().unwrap(),
        },
        beneficiary_id: registry.id().clone(),
        force,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify the account no longer exists
    worker.view_account(&market_id).await.unwrap_err();
}

#[rstest]
#[tokio::test]
async fn market_remove_nonexistent(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(true, false)] force: bool,
) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, beneficiary);

    // Use a non-existent account ID.
    let fake_id: AccountId = "nonexistent.test.near".parse().unwrap();
    let fake_signer = SignerArgs::new(
        fake_id,
        // Any valid secret key — the account doesn't exist so we won't actually sign.
        "ed25519:3D4YudUahN1nawWogh8pAKSj92sUNMdbZGjn7PnUXxXxKMCWMj3yMJTPqBZRiKjrBUrMp5MUP584vfmJhJTCMb8o"
            .parse()
            .unwrap(),
    );

    // Should succeed (no-op) when the account doesn't exist.
    MarketRemove {
        signer: fake_signer,
        beneficiary_id: beneficiary.id().clone(),
        force,
    }
    .run(&ctx)
    .await
    .unwrap();
}
