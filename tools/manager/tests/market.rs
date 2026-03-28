mod common;

use common::{setup_ctx, signer_args, write_json_file};
use near_sdk::{AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    registry::DeployMode,
};
use templar_manager::commands::{
    deployment::{FromRegistry, StandardDeploy},
    json_input::ArgsSource,
    market::{
        deploy::{DeployMarket, MarketInitArgs},
        remove::MarketRemove,
    },
    registry::{
        deploy::DeployRegistry,
        version::add::{AddVersion, Package},
    },
    ContractWasm, FixedContractWasm, SignerArgs,
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

    let init_args = MarketInitArgs {
        configuration: config.clone(),
    };

    DeployMarket {
        deploy: StandardDeploy::native(
            signer_args(&market_account),
            ContractWasm::fixed(FixedContractWasm { no_build: true }),
            ArgsSource::inline(serde_json::to_string(&init_args).unwrap()),
        ),
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
async fn market_deploy_from_init_args_file(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, market_account, oracle, borrow, collateral, protocol);

    let config = market_configuration(
        oracle.id().clone(),
        borrow.id().clone(),
        collateral.id().clone(),
        protocol.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let init_args = MarketInitArgs {
        configuration: config.clone(),
    };
    let init_args_file = write_json_file("market-init-args", &init_args);

    DeployMarket {
        deploy: StandardDeploy::native(
            signer_args(&market_account),
            ContractWasm::fixed(FixedContractWasm { no_build: true }),
            ArgsSource::from_file(init_args_file.clone()),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    let stored_config: MarketConfiguration = ctx
        .near
        .view(market_account.id(), "get_configuration")
        .await
        .unwrap()
        .json()
        .unwrap();

    assert_eq!(stored_config, config);

    std::fs::remove_file(init_args_file).unwrap();
}

#[rstest]
#[tokio::test]
async fn market_create_from_registry_and_removal(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);

    accounts!(worker, registry, oracle, borrow, collateral, protocol);

    let registry_signer = signer_args(&registry);

    DeployRegistry {
        deploy: StandardDeploy::native(
            registry_signer.clone(),
            ContractWasm::fixed(FixedContractWasm { no_build: true }),
            ArgsSource::inline("{}".to_string()),
        ),
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

    DeployMarket {
        deploy: StandardDeploy::from_registry(
            registry_signer.clone(),
            FromRegistry::new(
                registry.id().clone(),
                "market@test".to_string(),
                "mkt".to_string(),
            )
            .with_deposit(NearToken::from_near(6)),
            ArgsSource::inline(
                serde_json::to_string(&MarketInitArgs {
                    configuration: config.clone(),
                })
                .unwrap(),
            ),
        ),
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
        force: true,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify the account no longer exists
    let e = worker.view_account(&market_id).await.unwrap_err();
    assert!(e
        .into_inner()
        .unwrap()
        .to_string()
        .contains("does not exist while viewing"));
}

#[rstest]
#[tokio::test]
async fn market_create_from_registry_with_configuration_file(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let ctx = setup_ctx(&worker);

    accounts!(worker, registry, oracle, borrow, collateral, protocol);

    let registry_signer = signer_args(&registry);

    DeployRegistry {
        deploy: StandardDeploy::native(
            registry_signer.clone(),
            ContractWasm::fixed(FixedContractWasm { no_build: true }),
            ArgsSource::inline("{}".to_string()),
        ),
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

    let config = market_configuration(
        oracle.id().clone(),
        borrow.id().clone(),
        collateral.id().clone(),
        protocol.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );
    let init_args_file = write_json_file(
        "market-configuration",
        &MarketInitArgs {
            configuration: config.clone(),
        },
    );

    DeployMarket {
        deploy: StandardDeploy::from_registry(
            registry_signer.clone(),
            FromRegistry::new(
                registry.id().clone(),
                "market@test".to_string(),
                "mkt-file".to_string(),
            )
            .with_deposit(NearToken::from_near(6)),
            ArgsSource::from_file(init_args_file.clone()),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    let market_id: AccountId = format!("mkt-file.{}", registry.id()).parse().unwrap();

    let stored_config: MarketConfiguration = ctx
        .near
        .view(&market_id, "get_configuration")
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(stored_config, config);

    std::fs::remove_file(init_args_file).unwrap();
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
