#![allow(clippy::unwrap_used)]
mod common;

use common::{no_build_loader, setup_ctx, signer_args, view_json};
use near_sdk::{serde_json::json, AccountId, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{
    oracle::{pyth::PriceIdentifier, redstone::FeedId},
    registry::DeployMode,
    time::Nanoseconds,
};
use templar_manager::commands::{
    deployment::{Deploy, FromRegistry},
    proxy_oracle::{
        deploy::DeployProxyOracle,
        governance::{
            cancel::CancelProposal,
            create::{CreateProposal, OperationCommand, ProxyArgs, SetTtlArgs},
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
};
use templar_manager::util::{EmptyArgsLoader, OutputArgs};
use templar_proxy_oracle_kernel::{
    proxy::{Aggregator, FreshnessFilter, Proxy},
    request::OracleRequest,
};
use test_utils::{accounts, worker};

fn sample_price_id() -> CliPriceIdentifier {
    "0000000000000000000000000000000000000000000000000000000000000001"
        .parse()
        .unwrap()
}

fn sample_proxy(oracle_id: AccountId) -> Proxy {
    Proxy::new(
        Aggregator::median_low([OracleRequest::redstone(oracle_id, FeedId::from("ETH")).into()]),
        FreshnessFilter::empty(),
    )
}

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
        execute_immediately: false,
    }
    .run(&ctx)
    .await
    .unwrap();

    // List proposals — should have 1.
    let ids: Vec<u32> = view_json(&ctx, &oracle_id, "gov_list", json!({})).await;
    assert_eq!(ids, vec![0]);

    // Get proposal details (command should succeed).
    GetProposal {
        oracle_id: oracle_id.clone(),
        id: 0,
        output: OutputArgs::default(),
    }
    .run(&ctx)
    .await
    .unwrap();

    // List proposals command should succeed.
    ListProposals {
        oracle_id: oracle_id.clone(),
        output: OutputArgs::default(),
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
    let ids: Vec<u32> = view_json(&ctx, &oracle_id, "gov_list", json!({})).await;
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
        output: OutputArgs::default(),
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
        execute_immediately: false,
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
    let ids: Vec<u32> = view_json(&ctx, &oracle_id, "gov_list", json!({})).await;
    assert!(ids.is_empty());

    let new_ttl: Nanoseconds = view_json(&ctx, &oracle_id, "gov_ttl_ns", json!({})).await;
    assert_eq!(new_ttl, Nanoseconds::from_ms(5000));
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_governance_create_and_execute_immediately(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);
    let oracle_id = oracle.id().clone();

    deploy_proxy_oracle(&ctx, &oracle).await;

    CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(0),
        operation: OperationCommand::SetTtl(SetTtlArgs::from_ms(5000)),
        execute_immediately: true,
    }
    .run(&ctx)
    .await
    .unwrap();

    let ids: Vec<u32> = view_json(&ctx, &oracle_id, "gov_list", json!({})).await;
    assert!(ids.is_empty());

    let new_ttl: Nanoseconds = view_json(&ctx, &oracle_id, "gov_ttl_ns", json!({})).await;
    assert_eq!(new_ttl, Nanoseconds::from_ms(5000));
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_governance_create_and_execute_immediately_requires_zero_ttl(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);
    let oracle_id = oracle.id().clone();

    deploy_proxy_oracle(&ctx, &oracle).await;

    CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(0),
        operation: OperationCommand::SetTtl(SetTtlArgs::from_ms(5000)),
        execute_immediately: true,
    }
    .run(&ctx)
    .await
    .unwrap();

    let err = CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(1),
        operation: OperationCommand::SetTtl(SetTtlArgs::from_ms(1000)),
        execute_immediately: true,
    }
    .run(&ctx)
    .await
    .unwrap_err();

    assert!(err
        .to_string()
        .contains("cannot immediately execute proposal 1"));

    let ids: Vec<u32> = view_json(&ctx, &oracle_id, "gov_list", json!({})).await;
    assert_eq!(ids, vec![1]);

    let ttl: Nanoseconds = view_json(&ctx, &oracle_id, "gov_ttl_ns", json!({})).await;
    assert_eq!(ttl, Nanoseconds::from_ms(5000));
}

#[rstest]
#[tokio::test]
async fn proxy_oracle_governance_proxy_action_flags(#[future(awt)] worker: Worker<Sandbox>) {
    let ctx = setup_ctx(&worker);
    accounts!(worker, oracle);
    let oracle_id = oracle.id().clone();
    let price_id = sample_price_id();

    deploy_proxy_oracle(&ctx, &oracle).await;

    let proxy = sample_proxy(oracle_id.clone());

    CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(0),
        operation: OperationCommand::Proxy(ProxyArgs::insert(
            price_id,
            serde_json::to_string(&proxy).unwrap(),
        )),
        execute_immediately: true,
    }
    .run(&ctx)
    .await
    .unwrap();

    let get_proxy = view_json::<Option<Proxy>>(
        &ctx,
        &oracle_id,
        "get_proxy",
        json!({ "id": PriceIdentifier::from(price_id) }),
    )
    .await
    .unwrap();

    assert_eq!(get_proxy, proxy);

    CreateProposal {
        signer: signer_args(&oracle),
        oracle_id: oracle_id.clone(),
        id: Some(1),
        operation: OperationCommand::Proxy(ProxyArgs::remove(price_id)),
        execute_immediately: true,
    }
    .run(&ctx)
    .await
    .unwrap();

    let get_proxy: Option<Proxy> = view_json(
        &ctx,
        &oracle_id,
        "get_proxy",
        json!({ "id": PriceIdentifier::from(price_id) }),
    )
    .await;

    assert_eq!(get_proxy, None);
}
