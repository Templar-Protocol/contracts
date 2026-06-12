#![allow(clippy::unwrap_used)]
mod common;

use common::{no_build_loader, setup_ctx, signer_args, view_json};
use near_sdk::{serde_json::json, AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::registry::DeployMode;
use templar_manager::commands::{
    deployment::{Deploy, FromRegistry},
    proxy_oracle::{
        deploy::DeployProxyOracle,
        proxy::{get::GetProxy, list::ListProxies, CliPriceIdentifier},
        remove::ProxyOracleRemove,
    },
    registry::{
        deploy::DeployRegistry,
        version::add::{AddVersion, Package},
    },
};
use templar_manager::util::{EmptyArgsLoader, OutputArgs};
use test_utils::{accounts, worker};

/// Helper: deploy a proxy oracle on the given account.
async fn deploy_proxy_oracle(
    ctx: &templar_manager::CliContext,
    account: &near_workspaces::Account,
) {
    DeployProxyOracle {
        deploy: Deploy::native(
            signer_args(account),
            no_build_loader(),
            EmptyArgsLoader::default(),
        ),
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
async fn proxy_oracle_deploy(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);

    deploy_proxy_oracle(&ctx, &oracle).await;

    worker.view_account(oracle.id()).await.unwrap();

    // Verify contract responds to view call.
    let proxies: Vec<serde_json::Value> =
        view_json(&ctx, oracle.id(), "list_proxies", json!({})).await;
    assert!(proxies.is_empty());
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_create_from_registry(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let registry_id = registry.id().clone();
    let registry_signer = signer_args(&registry);

    DeployRegistry {
        deploy: Deploy::native(
            registry_signer.clone(),
            no_build_loader(),
            EmptyArgsLoader::default(),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    AddVersion {
        signer: registry_signer.clone(),
        contract_wasm: no_build_loader(),
        package: Package {
            market: false,
            uac: false,
            proxy_oracle: true,
            redstone_adapter: false,
            package: None,
        },
        registry_id: registry_id.clone(),
        version_key: Some("proxy-oracle@test".to_string()),
        deploy_mode: DeployMode::Normal,
        deposit: None,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Create proxy oracle from registry. The registry's deploy is owner-only.
    DeployProxyOracle {
        deploy: Deploy::from_registry(
            FromRegistry::new(
                registry_id.clone(),
                "proxy-oracle@test".to_string(),
                "po".to_string(),
                EmptyArgsLoader::default(),
                registry_signer.clone(),
            )
            .with_deposit(NearToken::from_near(6)),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    let oracle_id: AccountId = format!("po.{registry_id}").parse().unwrap();
    worker.view_account(&oracle_id).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_remove(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle, beneficiary);
    let oracle_id = oracle.id().clone();

    deploy_proxy_oracle(&ctx, &oracle).await;
    worker.view_account(&oracle_id).await.unwrap();

    ProxyOracleRemove {
        signer: signer_args(&oracle),
        beneficiary_id: beneficiary.id().clone(),
    }
    .run(&ctx)
    .await
    .unwrap();

    worker.view_account(&oracle_id).await.unwrap_err();
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_proxy_list_empty(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);

    deploy_proxy_oracle(&ctx, &oracle).await;

    // Should succeed with empty results.
    ListProxies {
        oracle_id: oracle.id().clone(),
        output: OutputArgs::default(),
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_get_proxy_not_found(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);

    deploy_proxy_oracle(&ctx, &oracle).await;

    // Query a nonexistent proxy — should succeed (prints "not found").
    let price_id: CliPriceIdentifier =
        "0000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .unwrap();
    GetProxy {
        oracle_id: oracle.id().clone(),
        price_id,
        output: OutputArgs::default(),
    }
    .run(&ctx)
    .await
    .unwrap();
}
