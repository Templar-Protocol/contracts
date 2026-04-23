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
            body: registry::AddVersionBody {
                registry_id: registry_id.clone(),
                version_key: version_key.clone(),
                deploy_mode: templar_common::registry::DeployMode::Normal,
                code: Base64Bytes(stack.harness.ft_wasm().await),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;
    eprintln!("{write_result:?}");

    let versions = stack
        .controller
        .request::<registry::ListVersions>(&ReadRequest {
            params: registry::ListVersionsParams {
                registry_id: registry_id.clone(),
                args: templar_gateway_types::common::Pagination::default(),
            },
        })
        .await?;

    assert_eq!(versions.values, vec![version_key.clone()]);

    let deploy = stack
        .controller
        .request::<registry::Deploy>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::DeployBody {
                registry_id: registry_id.clone(),
                name: "deployed-ft".to_owned(),
                version_key: version_key.clone(),
                init_args: Base64Bytes(serde_json::to_vec(&serde_json::json!({
                    "name": "Deployed FT",
                    "symbol": "DFT",
                }))?),
                full_access_keys: None,
                deposit: NearToken::from_near(6),
            },
        })
        .await?;

    let deployed_account_id: near_account_id::AccountId = format!("deployed-ft.{}", registry_id.0)
        .parse()
        .expect("deployed registry subaccount should be valid");

    let deployment = stack
        .controller
        .request::<registry::GetDeployment>(&ReadRequest {
            params: registry::GetDeploymentParams {
                registry_id: registry_id.clone(),
                account_id: deployed_account_id.clone(),
            },
        })
        .await?;

    let deployments = stack
        .controller
        .request::<registry::ListDeployments>(&ReadRequest {
            params: registry::ListDeploymentsParams {
                registry_id: registry_id.clone(),
                args: templar_gateway_types::common::Pagination::default(),
            },
        })
        .await?;

    let markets_only = stack
        .controller
        .request::<registry::ListDeploymentsByKind>(&ReadRequest {
            params: registry::ListDeploymentsByKindParams {
                registry_id: registry_id.clone(),
                args: templar_gateway_types::common::Pagination::default(),
                kind: contract::ContractKind::Market,
            },
        })
        .await?;

    let unknown_only = stack
        .controller
        .request::<registry::ListDeploymentsByKind>(&ReadRequest {
            params: registry::ListDeploymentsByKindParams {
                registry_id: registry_id.clone(),
                args: templar_gateway_types::common::Pagination::default(),
                kind: contract::ContractKind::Unknown,
            },
        })
        .await?;

    let version = stack
        .controller
        .request::<contract::GetVersion>(&ReadRequest {
            params: contract::GetVersionParams {
                contract_id: deployed_account_id,
            },
        })
        .await?;

    let _ = stack
        .controller
        .request::<registry::RemoveVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::RemoveVersionBody {
                registry_id: registry_id.clone(),
                version_key: version_key.clone(),
            },
        })
        .await?;

    assert_eq!(
        deployments.account_ids,
        vec![format!("deployed-ft.{}", registry_id.0).parse::<near_account_id::AccountId>()?]
    );
    assert!(deployment.deployment.is_some());
    assert!(!version.version_string.is_empty());
    assert!(markets_only.account_ids.is_empty());
    assert_eq!(unknown_only.account_ids, deployments.account_ids);
    assert_eq!(
        deploy.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    stack.shutdown().await;
    Ok(())
}
