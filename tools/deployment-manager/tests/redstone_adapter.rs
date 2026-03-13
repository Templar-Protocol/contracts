#![allow(clippy::unwrap_used)]
mod common;

use base64::prelude::*;
use common::{setup_ctx, signer_args};
use hex_literal::hex;
use near_sdk::{serde_json::json, AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{oracle::redstone::Config, registry::DeployMode};
use templar_deployment_manager::commands::{
    redstone_adapter::{
        config::AdapterConfig,
        create::{ConfigSource, CreateRedStoneAdapter},
        deploy::DeployRedStoneAdapter,
        feed::get::FeedGet,
        remove::RedStoneAdapterRemove,
        role::{list::RoleList, set::RoleSet},
        write_prices::WritePrices,
        CliRole,
    },
    registry::{
        deploy::DeployRegistry,
        version::add::{AddVersion, Package},
    },
    DeployFromRegistry, FixedContractWasm,
};
use test_utils::{accounts, worker};

/// Stellar test payload containing ETH + BTC prices, timestamp `1_770_985_144_000` ms.
/// Signed by production RedStone signers.
const STELLAR_PAYLOAD: &[u8] = &hex!("45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9030a710019c56f0bec0000000200000015d1cb1a708c63264741b00ce097176e45f708914b8cfdca26b079877a70604e25aa0bcfa3a41df8212eddd51db3496b95c7c3dc4caa9ac9705602af0515db1b31c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec000000020000001dcaf484941c0d206f1898185b953c6a92d7fd188b347505c0f5beb2030e06e3e1b2f7dfb45929ac7676136af93fee7f14a614b40fa4dc2d1e625dbece02eaca21c45544800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002d9028ed04019c56f0bec00000002000000199bd54930138268baad2869e9ceb99b6bc67cd6b8a4cc98e05f0b1cd9b7f07066008208399a728fac3d1dc3ca407cb8199a0209377bceb0c48f2cc3d756078051b4254430000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006179a92ab8c019c56f0bec000000020000001f08af53ed34046f7f64cc02ffb7973252954d7c395e440693c896bffdbc2de1e31cf5675bf66583d3e3438f5002ae9c10870d4dc45de05c560b239aa3a2d50a41b425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec0000000200000011b96dc2763a692e3245ce4f1b0c16ea245c240204e99ebd323b340e58bfb14fb5f0465ce11b8dd52ff839547cc949d20e4e8ba0be43dd6417cade2a8ebfd8c9e1c425443000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000617a1187473019c56f0bec00000002000000114a02710892325b13afc74bbd350dd9ec80342b2d6c0c94df7b7a60dbf67a1b91b182fa4555e0e0db91e6258b279f00b7eeb8f5de9930e352d5321a6b8b64a031c00063137373039383531343539383223302e392e30237374656c6c61722d636f6e6e6563746f72000025000002ed57011e0000");

fn test_config_source() -> ConfigSource {
    ConfigSource {
        prod: false,
        test: true,
        configuration: None,
    }
}

/// Config using prod signers (matching the test payloads) but with a very large
/// timestamp tolerance so old payloads are accepted regardless of sandbox time.
fn prod_config_with_relaxed_timestamps() -> ConfigSource {
    let mut config = templar_common::oracle::redstone::config::prod();
    config.max_timestamp_delay_ms = 365 * 24 * 60 * 60 * 1000; // 1 year
    config.max_timestamp_ahead_ms = 365 * 24 * 60 * 60 * 1000;
    config.min_interval_between_updates_ms = 0;
    ConfigSource {
        prod: false,
        test: false,
        configuration: Some(serde_json::to_value(&config).unwrap()),
    }
}

/// Helper: deploy a RedStone adapter on the given account.
async fn deploy_adapter(
    ctx: &templar_deployment_manager::CliContext,
    account: &near_workspaces::Account,
) {
    DeployRedStoneAdapter {
        signer: signer_args(account),
        contract_wasm: FixedContractWasm { no_build: true },
        config_source: test_config_source(),
    }
    .run(ctx)
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[rstest]
#[tokio::test]
async fn redstone_adapter_deploy(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, adapter);

    deploy_adapter(&ctx, &adapter).await;

    worker.view_account(adapter.id()).await.unwrap();

    // Verify config view call works.
    let config: Config = ctx
        .near()
        .view(adapter.id(), "get_config")
        .await
        .unwrap()
        .json()
        .unwrap();

    // The test config should have some signers configured.
    assert!(!config.signers.is_empty());
}

#[rstest]
#[tokio::test]
async fn redstone_adapter_create_from_registry(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let registry_id = registry.id().clone();
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
            market: false,
            uac: false,
            proxy_oracle: false,
            redstone_adapter: true,
            package: None,
        },
        registry_id: registry_id.clone(),
        version_key: Some("redstone@test".to_string()),
        deploy_mode: DeployMode::Normal,
        deposit: None,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Create adapter from registry. The registry's deploy is owner-only.
    CreateRedStoneAdapter {
        signer: registry_signer.clone(),
        deploy: DeployFromRegistry {
            registry_id: registry_id.clone(),
            version_key: "redstone@test".to_string(),
            name: "rs".to_string(),
            with_full_access_key: vec![],
            no_signer_full_access_key: false,
            deposit: Some(NearToken::from_near(6)),
        },
        config_source: test_config_source(),
    }
    .run(&ctx)
    .await
    .unwrap();

    let adapter_id: AccountId = format!("rs.{registry_id}").parse().unwrap();
    worker.view_account(&adapter_id).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn redstone_adapter_remove(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, adapter, beneficiary);
    let adapter_id = adapter.id().clone();

    deploy_adapter(&ctx, &adapter).await;
    worker.view_account(&adapter_id).await.unwrap();

    RedStoneAdapterRemove {
        signer: signer_args(&adapter),
        beneficiary_id: beneficiary.id().clone(),
    }
    .run(&ctx)
    .await
    .unwrap();

    worker.view_account(&adapter_id).await.unwrap_err();
}

#[rstest]
#[tokio::test]
async fn redstone_adapter_config(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, adapter);

    deploy_adapter(&ctx, &adapter).await;

    // The config command should succeed.
    AdapterConfig {
        adapter_id: adapter.id().clone(),
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn redstone_adapter_role_lifecycle(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, adapter, target);
    let adapter_id = adapter.id().clone();
    let target_id = target.id().clone();

    deploy_adapter(&ctx, &adapter).await;

    // Grant TrustedUpdater role.
    RoleSet {
        signer: signer_args(&adapter),
        adapter_id: adapter_id.clone(),
        target_account_id: target_id.clone(),
        role: CliRole::TrustedUpdater,
        revoke: false,
    }
    .run(&ctx)
    .await
    .unwrap();

    // List role members — target should appear.
    let members: Vec<AccountId> = ctx
        .near()
        .view(&adapter_id, "list_role")
        .args_json(json!({ "role": templar_common::oracle::redstone::Role::TrustedUpdater }))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(members.contains(&target_id));

    // RoleList command should succeed.
    RoleList {
        adapter_id: adapter_id.clone(),
        role: CliRole::TrustedUpdater,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Revoke the role.
    RoleSet {
        signer: signer_args(&adapter),
        adapter_id: adapter_id.clone(),
        target_account_id: target_id.clone(),
        role: CliRole::TrustedUpdater,
        revoke: true,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify the role is gone.
    let members: Vec<AccountId> = ctx
        .near()
        .view(&adapter_id, "list_role")
        .args_json(json!({ "role": templar_common::oracle::redstone::Role::TrustedUpdater }))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(!members.contains(&target_id));
}

#[rstest]
#[tokio::test]
async fn redstone_adapter_feed_get_empty(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, adapter);

    deploy_adapter(&ctx, &adapter).await;

    // Query feed data before any prices are written — should succeed with empty result.
    FeedGet {
        adapter_id: adapter.id().clone(),
        feed_id: vec!["ETH".to_string()],
        json: false,
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn redstone_adapter_write_prices(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, adapter);

    // Deploy with prod signers (matching the payload) but relaxed timestamp tolerance.
    DeployRedStoneAdapter {
        signer: signer_args(&adapter),
        contract_wasm: FixedContractWasm { no_build: true },
        config_source: prod_config_with_relaxed_timestamps(),
    }
    .run(&ctx)
    .await
    .unwrap();

    // Write prices using the stellar test payload (ETH + BTC).
    WritePrices {
        signer: signer_args(&adapter),
        adapter_id: adapter.id().clone(),
        feed_id: vec!["ETH".into(), "BTC".into()],
        payload: BASE64_STANDARD.encode(STELLAR_PAYLOAD),
    }
    .run(&ctx)
    .await
    .unwrap();

    // Read back prices via FeedGet command — should succeed with data present.
    FeedGet {
        adapter_id: adapter.id().clone(),
        feed_id: vec!["ETH".to_string(), "BTC".to_string()],
        json: true,
    }
    .run(&ctx)
    .await
    .unwrap();
}
