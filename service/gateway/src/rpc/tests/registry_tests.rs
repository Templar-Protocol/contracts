use near_account_id::AccountId;
use near_sdk::json_types::Base58CryptoHash;
use templar_common::registry::VersionSource;
use templar_gateway_types::{common::Pagination, ContractKind, OperationStatus};

use super::*;

#[tokio::test]
async fn registry_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;

    let version_key = "mock-ft@1.0.0".to_owned();
    let write_result = stack
        .controller
        .request::<registry::AddVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::AddVersion {
                registry_id: registry_id.clone(),
                version_key: version_key.clone(),
                source: VersionSource::Stored(stack.harness.ft_wasm().await.into()),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;
    eprintln!("{write_result:?}");

    let versions = stack
        .controller
        .request::<registry::ListVersions>(&registry::ListVersions {
            registry_id: registry_id.clone(),
            args: Pagination::default(),
        })
        .await?;

    assert_eq!(versions.values, vec![version_key.clone()]);

    let deploy = stack
        .controller
        .request::<registry::Deploy>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::Deploy {
                target: registry::DeployTarget {
                    registry_id: registry_id.clone(),
                    name: "deployed-ft".to_owned(),
                    version_key: version_key.clone(),
                    full_access_keys: None,
                    deposit: NearToken::from_near(6),
                },
                init_args: Base64Bytes(serde_json::to_vec(&serde_json::json!({
                    "name": "Deployed FT",
                    "symbol": "DFT",
                }))?),
            },
        })
        .await?;

    let deployed_account_id: AccountId = format!("deployed-ft.{registry_id}")
        .parse()
        .expect("deployed registry subaccount should be valid");

    let deployment = stack
        .controller
        .request::<registry::GetDeployment>(&registry::GetDeployment {
            registry_id: registry_id.clone(),
            account_id: deployed_account_id.clone(),
        })
        .await?;

    let deployments = stack
        .controller
        .request::<registry::ListDeployments>(&registry::ListDeployments {
            registry_id: registry_id.clone(),
            args: Pagination::default(),
        })
        .await?;

    let markets_only = stack
        .controller
        .request::<registry::ListDeploymentsByKind>(&registry::ListDeploymentsByKind {
            registry_id: registry_id.clone(),
            args: Pagination::default(),
            kind: ContractKind::Market,
        })
        .await?;

    let unknown_only = stack
        .controller
        .request::<registry::ListDeploymentsByKind>(&registry::ListDeploymentsByKind {
            registry_id: registry_id.clone(),
            args: Pagination::default(),
            kind: ContractKind::Unknown,
        })
        .await?;

    let version = stack
        .controller
        .request::<contract::GetVersion>(&contract::GetVersion {
            contract_id: deployed_account_id,
        })
        .await?;

    let _ = stack
        .controller
        .request::<registry::RemoveVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::RemoveVersion {
                registry_id: registry_id.clone(),
                version_key: version_key.clone(),
            },
        })
        .await?;

    assert_eq!(
        deployments.account_ids,
        vec![format!("deployed-ft.{registry_id}").parse::<AccountId>()?]
    );
    assert!(deployment.deployment.is_some());
    assert!(!version.version_string.is_empty());
    assert!(markets_only.account_ids.is_empty());
    assert_eq!(unknown_only.account_ids, deployments.account_ids);
    assert_eq!(deploy.operation.status, OperationStatus::Succeeded);

    stack.shutdown().await;
    Ok(())
}

/// Every `VersionSource` must survive the round trip through the gateway and produce a version
/// `registry.deploy` can use — including `ExistingGlobal`, which stakes nothing because the code is
/// already on chain.
#[tokio::test]
async fn add_version_accepts_every_source_and_each_one_deploys() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;
    let wasm = stack.harness.ft_wasm().await;
    let publish_deposit = templar_gateway_testing::publish_deposit_for(wasm.len());

    let add = async |version_key: &str, source: VersionSource, deposit: NearToken| {
        stack
            .controller
            .request::<registry::AddVersion>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                body: registry::AddVersion {
                    registry_id: registry_id.clone(),
                    version_key: version_key.to_owned(),
                    source,
                    deposit,
                },
            })
            .await
    };

    add(
        "stored@1.0.0",
        VersionSource::Stored(wasm.clone().into()),
        NearToken::from_yoctonear(1),
    )
    .await?;
    add(
        "published@1.0.0",
        VersionSource::PublishGlobal(wasm.clone().into()),
        publish_deposit,
    )
    .await?;

    // The hash the registry recorded for the blob it just published is exactly what a second
    // registry would need in order to serve the same code without paying for it again. Read it
    // through the raw view: `registry.getVersion` is gated on a release these sandbox registries
    // are not yet built as.
    let published: Base58CryptoHash = serde_json::from_value(
        view_contract_json(
            &stack,
            registry_id.clone(),
            "get_version_code_hash",
            serde_json::json!({ "version_key": "published@1.0.0" }),
        )
        .await?,
    )?;

    add(
        "by-hash@1.0.0",
        VersionSource::ExistingGlobal(published),
        NearToken::from_yoctonear(1),
    )
    .await?;

    for (name, version_key) in [
        ("from-stored", "stored@1.0.0"),
        ("from-published", "published@1.0.0"),
        ("from-hash", "by-hash@1.0.0"),
    ] {
        let deploy = stack
            .controller
            .request::<registry::Deploy>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                body: registry::Deploy {
                    target: registry::DeployTarget {
                        registry_id: registry_id.clone(),
                        name: name.to_owned(),
                        version_key: version_key.to_owned(),
                        full_access_keys: None,
                        deposit: NearToken::from_near(6),
                    },
                    init_args: Base64Bytes(serde_json::to_vec(&serde_json::json!({
                        "name": "Deployed FT",
                        "symbol": "DFT",
                    }))?),
                },
            })
            .await?;

        assert_eq!(
            deploy.operation.status,
            OperationStatus::Succeeded,
            "{version_key} should deploy",
        );
    }

    stack.shutdown().await;
    Ok(())
}
