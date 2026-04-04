mod common;

use common::{no_build_loader, setup_ctx, signer_args, view_json, TestArgsKind};
use near_sdk::{AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{
    market::{MarketConfiguration, YieldWeights},
    registry::DeployMode,
};
use templar_manager::commands::{
    deployment::{Deploy, FromRegistry},
    market::{
        deploy::{DeployMarket, MarketInitArgs},
        remove::MarketRemove,
    },
    registry::{
        deploy::DeployRegistry,
        version::add::{AddVersion, Package},
    },
};
use templar_manager::util::{EmptyArgsLoader, GeneralArgsLoader, SignerArgs};
use test_utils::{accounts, market_configuration, worker};

async fn deploy_registry_with_market_version(
    ctx: &templar_manager::CliContext,
    registry_id: AccountId,
    registry_signer: SignerArgs,
) -> anyhow::Result<()> {
    DeployRegistry {
        deploy: Deploy::native(
            registry_signer.clone(),
            no_build_loader(),
            EmptyArgsLoader::default(),
        ),
    }
    .run(ctx)
    .await?;

    AddVersion {
        signer: registry_signer,
        contract_wasm: no_build_loader(),
        package: Package {
            market: true,
            uac: false,
            proxy_oracle: false,
            redstone_adapter: false,
            package: None,
        },
        registry_id,
        version_key: Some("market@test".to_string()),
        deploy_mode: DeployMode::Normal,
        deposit: None,
    }
    .run(ctx)
    .await
}

#[rstest]
#[case::inline(TestArgsKind::Inline)]
#[case::file(TestArgsKind::File)]
#[tokio::test]
async fn market_deploy(#[future(awt)] worker: Worker<Sandbox>, #[case] input_kind: TestArgsKind) {
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
    let args = input_kind.into_fixture("market-init-args", init_args);

    DeployMarket {
        deploy: Deploy::native(
            signer_args(&market_account),
            no_build_loader(),
            args.loader(),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify the contract is deployed by querying its configuration.
    let stored_config: MarketConfiguration = view_json(
        &ctx,
        market_account.id(),
        "get_configuration",
        serde_json::json!({}),
    )
    .await;

    assert_eq!(stored_config, config);
}

#[rstest]
#[case::inline(TestArgsKind::Inline, "mkt-inline")]
#[case::file(TestArgsKind::File, "mkt-file")]
#[tokio::test]
async fn market_create_from_registry(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] input_kind: TestArgsKind,
    #[case] market_name: &str,
) {
    let ctx = setup_ctx(&worker);

    accounts!(worker, registry, oracle, borrow, collateral, protocol);

    let registry_signer = signer_args(&registry);

    deploy_registry_with_market_version(&ctx, registry.id().clone(), registry_signer.clone())
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
    let init_args = MarketInitArgs {
        configuration: config.clone(),
    };
    let args = input_kind.into_fixture("market-configuration", init_args);

    DeployMarket {
        deploy: Deploy::from_registry(
            FromRegistry::new(
                registry.id().clone(),
                "market@test".to_string(),
                market_name.to_string(),
                args.loader(),
                registry_signer.clone(),
            )
            .with_deposit(NearToken::from_near(6)),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    let market_id: AccountId = format!("{market_name}.{}", registry.id()).parse().unwrap();

    // Verify market exists by querying configuration
    let stored_config: MarketConfiguration =
        view_json(&ctx, &market_id, "get_configuration", serde_json::json!({})).await;
    assert_eq!(stored_config, config);
}

#[rstest]
#[tokio::test]
async fn market_remove_after_registry_create(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);

    accounts!(worker, registry, oracle, borrow, collateral, protocol);

    let registry_signer = signer_args(&registry);

    deploy_registry_with_market_version(&ctx, registry.id().clone(), registry_signer.clone())
        .await
        .unwrap();

    let config = market_configuration(
        oracle.id().clone(),
        borrow.id().clone(),
        collateral.id().clone(),
        protocol.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    DeployMarket {
        deploy: Deploy::from_registry(
            FromRegistry::new(
                registry.id().clone(),
                "market@test".to_string(),
                "mkt-remove".to_string(),
                GeneralArgsLoader::from_json_string(
                    serde_json::to_string(&MarketInitArgs {
                        configuration: config,
                    })
                    .unwrap(),
                ),
                registry_signer.clone(),
            )
            .with_deposit(NearToken::from_near(6)),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    let market_id: AccountId = format!("mkt-remove.{}", registry.id()).parse().unwrap();

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

    let e = worker.view_account(&market_id).await.unwrap_err();
    assert!(e
        .into_inner()
        .unwrap()
        .to_string()
        .contains("does not exist while viewing"));
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
