#![allow(clippy::unwrap_used)]
mod common;

use common::{setup_ctx, signer_args};
use near_sdk::{serde_json::json, AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{registry::DeployMode, time::Nanoseconds};
use templar_manager::commands::{
    deployment::{FromRegistry, StandardDeploy},
    json_input::ArgsSource,
    proxy_oracle::{
        deploy::{DeployProxyOracle, ProxyOracleInitArgs},
        governance::{
            cancel::CancelProposal,
            create::{CreateProposal, OperationCommand, SetTtlArgs},
            execute::ExecuteProposal,
            get::GetProposal,
            list::ListProposals,
        },
        proxy::{get::GetProxy, list::ListProxies, CliPriceIdentifier},
        remove::ProxyOracleRemove,
    },
    registry::{
        deploy::DeployRegistry,
        version::add::{AddVersion, Package},
    },
    ContractWasm, FixedContractWasm,
};
use test_utils::{accounts, worker};

fn no_build() -> FixedContractWasm {
    FixedContractWasm { no_build: true }
}

/// Helper: deploy a proxy oracle on the given account.
async fn deploy_proxy_oracle(
    ctx: &templar_manager::CliContext,
    account: &near_workspaces::Account,
) {
    DeployProxyOracle {
        deploy: StandardDeploy::native(
            signer_args(account),
            ContractWasm::fixed(no_build()),
            ArgsSource::inline(serde_json::to_string(&ProxyOracleInitArgs {}).unwrap()),
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
    let proxies: Vec<serde_json::Value> = ctx
        .near
        .view(oracle.id(), "list_proxies")
        .args_json(json!({}))
        .await
        .unwrap()
        .json()
        .unwrap();
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
        deploy: StandardDeploy::native(
            registry_signer.clone(),
            ContractWasm::fixed(no_build()),
            ArgsSource::inline("{}".to_string()),
        ),
    }
    .run(&ctx)
    .await
    .unwrap();

    AddVersion {
        signer: registry_signer.clone(),
        contract_wasm: no_build(),
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
        deploy: StandardDeploy::from_registry(
            registry_signer.clone(),
            FromRegistry::new(
                registry_id.clone(),
                "proxy-oracle@test".to_string(),
                "po".to_string(),
            )
            .with_deposit(NearToken::from_near(6)),
            ArgsSource::inline(serde_json::to_string(&ProxyOracleInitArgs {}).unwrap()),
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
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_governance_lifecycle(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);
    let oracle_id = oracle.id().clone();

    deploy_proxy_oracle(&ctx, &oracle).await;

    // Create a SetTtl governance proposal.
    CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(0),
        operation: OperationCommand::SetTtl(SetTtlArgs::from_ms(1000)),
    }
    .run(&ctx)
    .await
    .unwrap();

    // List proposals — should have 1.
    let ids: Vec<u32> = ctx
        .near
        .view(&oracle_id, "gov_list")
        .args_json(json!({}))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(ids, vec![0]);

    // Get proposal details (command should succeed).
    GetProposal {
        oracle_id: oracle_id.clone(),
        id: 0,
    }
    .run(&ctx)
    .await
    .unwrap();

    // List proposals command should succeed.
    ListProposals {
        oracle_id: oracle_id.clone(),
    }
    .run(&ctx)
    .await
    .unwrap();

    // Cancel the proposal.
    CancelProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: 0,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify proposal is gone.
    let ids: Vec<u32> = ctx
        .near
        .view(&oracle_id, "gov_list")
        .args_json(json!({}))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(ids.is_empty());
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
        json: false,
    }
    .run(&ctx)
    .await
    .unwrap();
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_governance_execute(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);
    let oracle_id = oracle.id().clone();

    deploy_proxy_oracle(&ctx, &oracle).await;

    // Create a SetTtl governance proposal.
    CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(0),
        operation: OperationCommand::SetTtl(SetTtlArgs::from_ms(5000)),
    }
    .run(&ctx)
    .await
    .unwrap();

    // Execute the proposal.
    ExecuteProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: 0,
    }
    .run(&ctx)
    .await
    .unwrap();

    // Verify the proposal was executed (no longer in the list).
    let ids: Vec<u32> = ctx
        .near
        .view(&oracle_id, "gov_list")
        .args_json(json!({}))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(ids.is_empty());

    let new_ttl = ctx
        .near
        .view(&oracle_id, "gov_ttl_ns")
        .args_json(json!({}))
        .await
        .unwrap()
        .json::<Nanoseconds>()
        .unwrap();
    assert_eq!(new_ttl, Nanoseconds::from_ms(5000));
}
