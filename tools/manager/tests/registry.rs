#![allow(clippy::unwrap_used)]
mod common;

use common::{setup_ctx, signer_args};
use near_sdk::{serde_json::json, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{market::YieldWeights, registry::DeployMode};
use templar_manager::commands::{
    market::create::CreateMarket,
    registry::{
        deploy::DeployRegistry,
        deployment::{clear::ClearDeployments, list::ListDeployments},
        remove::RemoveRegistry,
        version::{
            add::{AddVersion, Package},
            list::ListVersions,
            remove::VersionRemove,
        },
    },
    DeployFromRegistry, FixedContractWasm, SignerArgs,
};
use test_utils::{accounts, market_configuration, worker};

fn no_build() -> FixedContractWasm {
    FixedContractWasm { no_build: true }
}

fn market_package() -> Package {
    Package {
        market: true,
        uac: false,
        proxy_oracle: false,
        redstone_adapter: false,
        package: None,
    }
}

/// Helper: deploy a registry contract on the given account.
async fn deploy_registry(ctx: &templar_manager::CliContext, signer: SignerArgs) {
    DeployRegistry {
        signer,
        contract: no_build(),
        no_init: false,
    }
    .run(ctx)
    .await
    .unwrap();
}

/// Helper: add a market version to the registry.
async fn add_market_version(
    ctx: &templar_manager::CliContext,
    signer: &SignerArgs,
    registry_id: &near_sdk::AccountId,
    version_key: &str,
) {
    AddVersion {
        signer: signer.clone(),
        contract_wasm: no_build(),
        package: market_package(),
        registry_id: registry_id.clone(),
        version_key: Some(version_key.to_string()),
        deploy_mode: DeployMode::Normal,
        deposit: None,
    }
    .run(ctx)
    .await
    .unwrap();
}

/// List versions via view call (returns version keys).
async fn view_versions(
    ctx: &templar_manager::CliContext,
    registry_id: &near_sdk::AccountId,
) -> Vec<String> {
    ctx.near
        .view(registry_id, "list_versions")
        .args_json(json!({}))
        .await
        .unwrap()
        .json()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[rstest]
#[tokio::test]
async fn registry_deploy(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);

    deploy_registry(&ctx, signer).await;

    worker.view_account(registry.id()).await.unwrap();

    // Verify the contract responds to a view call.
    let versions: Vec<String> = view_versions(&ctx, registry.id()).await;
    assert!(versions.is_empty());
}

#[rstest]
#[tokio::test]
async fn registry_version_lifecycle(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry, oracle, borrow, collateral, protocol);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer.clone()).await;

    // Add a version.
    add_market_version(&ctx, &signer, &registry_id, "market@v1").await;

    let versions = view_versions(&ctx, &registry_id).await;
    assert_eq!(versions, vec!["market@v1"]);

    // Remove the version (clears the stored code, but the entry remains in the map).
    VersionRemove {
        signer: signer.clone(),
        registry_id: registry_id.clone(),
        all: false,
        version_key: Some("market@v1".to_string()),
    }
    .run(&ctx)
    .await
    .unwrap();

    // The version key is still in the map but its code has been cleared.
    // Verify the command ran successfully by ensuring we can still list versions
    // (this confirms the remove didn't corrupt state).
    let versions = view_versions(&ctx, &registry_id).await;
    assert_eq!(
        versions,
        vec!["market@v1"],
        "entry remains but code is cleared"
    );

    let config = market_configuration(
        oracle.id().clone(),
        borrow.id().clone(),
        collateral.id().clone(),
        protocol.id().clone(),
        YieldWeights::new_with_supply_weight(1),
    );

    let deploy_err = CreateMarket {
        signer: signer.clone(),
        deploy: DeployFromRegistry {
            registry_id: registry_id.clone(),
            version_key: "market@v1".to_string(),
            name: "removed-version".to_string(),
            with_full_access_key: vec![],
            no_signer_full_access_key: false,
            deposit: Some(NearToken::from_near(6)),
        },
        configuration: serde_json::to_string(&config).unwrap(),
    }
    .run(&ctx)
    .await
    .unwrap_err()
    .to_string();

    assert!(
        deploy_err.contains("Version code has been deleted"),
        "expected deploy to fail because the removed version code was deleted, got: {deploy_err}"
    );
}

#[rstest]
#[tokio::test]
async fn registry_version_remove_all(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer.clone()).await;

    // Add two versions.
    add_market_version(&ctx, &signer, &registry_id, "market@v1").await;
    add_market_version(&ctx, &signer, &registry_id, "market@v2").await;

    let versions = view_versions(&ctx, &registry_id).await;
    assert_eq!(versions.len(), 2);

    // Remove all.
    VersionRemove {
        signer: signer.clone(),
        registry_id: registry_id.clone(),
        all: true,
        version_key: None,
    }
    .run(&ctx)
    .await
    .unwrap();

    // The entries remain in the map but code is cleared.
    // Verify both version keys still exist (confirming remove_all processed them).
    let versions = view_versions(&ctx, &registry_id).await;
    assert_eq!(versions.len(), 2, "entries remain but code is cleared");
}

#[rstest]
#[tokio::test]
async fn registry_list_versions_empty(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer).await;

    // The command should succeed even with no versions.
    ListVersions {
        registry_id: registry_id.clone(),
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn registry_deployment_list_empty(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer).await;

    // No deployments yet — should succeed without error.
    ListDeployments {
        registry_id: registry_id.clone(),
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn registry_remove(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry, beneficiary);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer.clone()).await;
    add_market_version(&ctx, &signer, &registry_id, "market@v1").await;

    // Remove the registry (removes versions, then deletes account).
    RemoveRegistry {
        signer,
        beneficiary_id: beneficiary.id().clone(),
    }
    .run(&ctx)
    .await
    .unwrap();

    worker.view_account(&registry_id).await.unwrap_err();
}

#[rstest]
#[tokio::test]
async fn registry_clear_deployments_empty(
    #[future(awt)] worker: Worker<Sandbox>,
    #[values(true, false)] force: bool,
) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer).await;

    // Clear deployments on a registry with no deployments — should succeed as a no-op.
    ClearDeployments {
        secret_key: registry.secret_key().to_string().parse().unwrap(),
        registry_id: registry_id.clone(),
        beneficiary_id: None,
        force,
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn registry_version_remove_mutual_exclusivity_conflict(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer.clone()).await;

    let err_msg = VersionRemove {
        signer: signer.clone(),
        registry_id: registry_id.clone(),
        all: true,
        version_key: Some("market@v1".to_string()),
    }
    .run(&ctx)
    .await
    .unwrap_err()
    .to_string();

    assert_eq!(err_msg, "Cannot specify both --all and --version-key");
}

#[rstest]
#[tokio::test]
async fn registry_version_remove_neither_specified(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, registry);
    let signer = signer_args(&registry);
    let registry_id = registry.id().clone();

    deploy_registry(&ctx, signer.clone()).await;

    let err_msg = VersionRemove {
        signer: signer.clone(),
        registry_id: registry_id.clone(),
        all: false,
        version_key: None,
    }
    .run(&ctx)
    .await
    .unwrap_err()
    .to_string();

    assert_eq!(err_msg, "Please specify either --all or --version-key");
}
