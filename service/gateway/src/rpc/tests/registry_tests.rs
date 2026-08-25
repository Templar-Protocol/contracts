use near_account_id::AccountId;
use near_sdk::json_types::Base58CryptoHash;
use templar_common::registry::VersionSource;
use templar_gateway_methods_spec::proxy_oracle;
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
                    skip_abi_check: true,
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
                        skip_abi_check: true,
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

fn wasm_with_no_arg_constructor_abi() -> Vec<u8> {
    let abi = r#"{"schema_version":"0.4.0","metadata":{},"body":{"functions":[{"name":"new","kind":"call","modifiers":["init"],"params":{"serialization_type":"json","args":[]}}],"root_schema":{"definitions":{}}}}"#;
    let compressed = zstd::stream::encode_all(abi.as_bytes(), 0).expect("compress ABI");
    let mut section = vec![1, 0, 0x41, 0, 0x0b];
    push_uleb(
        &mut section,
        u32::try_from(compressed.len()).expect("compressed ABI fits Wasm section length"),
    );
    section.extend(compressed);

    let mut wasm = b"\0asm\x01\0\0\0".to_vec();
    wasm.push(11);
    push_uleb(
        &mut wasm,
        u32::try_from(section.len()).expect("Wasm section fits Wasm section length"),
    );
    wasm.extend(section);
    wasm
}

async fn add_stored_abi_version(
    stack: &TestStack,
    registry_id: &AccountId,
    version_key: &str,
) -> Result<()> {
    stack
        .harness
        .registry_add_version(
            &stack.harness.registry_signer_account_id,
            registry_id,
            version_key,
            VersionSource::Stored(wasm_with_no_arg_constructor_abi().into()),
            NearToken::from_yoctonear(1),
        )
        .await?;
    Ok(())
}

fn push_uleb(bytes: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        bytes.push(byte);
        if value == 0 {
            return;
        }
    }
}

#[tokio::test]
async fn stored_and_global_versions_reject_invalid_constructor_args() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;
    let wasm = wasm_with_no_arg_constructor_abi();
    let versions = [
        (
            "uncatalogued@1.0.0",
            "invalid-init",
            VersionSource::Stored(wasm.clone().into()),
            NearToken::from_yoctonear(1),
        ),
        (
            "global@1.0.0",
            "invalid-global-init",
            VersionSource::PublishGlobal(wasm.clone().into()),
            templar_gateway_testing::publish_deposit_for(wasm.len()),
        ),
    ];

    for (version_key, name, source, deposit) in versions {
        stack
            .harness
            .registry_add_version(
                &stack.harness.registry_signer_account_id,
                &registry_id,
                version_key,
                source,
                deposit,
            )
            .await?;

        let error = stack
            .controller
            .request::<registry::Deploy>(&WriteRequest {
                signer_account_id: stack.harness.registry_signer_account_id.clone(),
                idempotency_key: None,
                body: registry::Deploy {
                    target: registry::DeployTarget {
                        registry_id: registry_id.clone(),
                        name: name.to_owned(),
                        version_key: version_key.to_owned(),
                        skip_abi_check: false,
                        full_access_keys: None,
                        deposit: NearToken::from_near(6),
                    },
                    init_args: Base64Bytes(br#"{"unexpected":true}"#.to_vec()),
                },
            })
            .await
            .expect_err("constructor ABI must reject arguments before deployment");
        assert!(format!("{error}").contains("do not match"));
    }

    let deployments = stack
        .controller
        .request::<registry::ListDeployments>(&registry::ListDeployments {
            registry_id,
            args: Pagination::default(),
        })
        .await?;
    assert!(deployments.account_ids.is_empty());

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn removed_version_is_rejected_before_deployment() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;
    let version_key = "removed@1.0.0".to_owned();
    add_stored_abi_version(&stack, &registry_id, &version_key).await?;
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

    let error = stack
        .controller
        .request::<registry::Deploy>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::Deploy {
                target: registry::DeployTarget {
                    registry_id,
                    name: "removed-version".to_owned(),
                    version_key,
                    skip_abi_check: false,
                    full_access_keys: None,
                    deposit: NearToken::from_near(6),
                },
                init_args: Base64Bytes(b"{}".to_vec()),
            },
        })
        .await
        .expect_err("removed version must fail before deployment planning");
    assert!(format!("{error}").contains("has been removed"));

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn proxy_oracle_owner_id_is_checked_against_constructor_abi() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;
    let version_key = "old-proxy@1.0.0".to_owned();
    add_stored_abi_version(&stack, &registry_id, &version_key).await?;

    let error = stack
        .controller
        .request::<proxy_oracle::Create>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: proxy_oracle::Create {
                target: registry::DeployTarget {
                    registry_id,
                    name: "old-proxy".to_owned(),
                    version_key,
                    skip_abi_check: false,
                    full_access_keys: None,
                    deposit: NearToken::from_near(6),
                },
                owner_id: Some("owner.near".parse()?),
            },
        })
        .await
        .expect_err("owner_id must be rejected by the zero-argument constructor ABI");
    assert!(format!("{error}").contains("do not match"));

    stack.shutdown().await;
    Ok(())
}
