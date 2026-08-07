use super::*;

#[tokio::test]
async fn redstone_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let oracle_id = stack
        .harness
        .deploy_mock_oracle("redstone-low-level")
        .await?;

    stack
        .harness
        .set_mock_oracle_redstone_price(oracle_id.clone(), "BTC".into(), Some(redstone_price(42.0)))
        .await?;

    let config = stack
        .controller
        .request::<redstone::GetConfig>(&redstone::GetConfig {
            oracle_id: oracle_id.clone(),
        })
        .await?;
    assert!(config.config.signer_count_threshold > 0);

    let prices = stack
        .controller
        .request::<redstone::ReadPriceData>(&redstone::ReadPriceData {
            oracle_id: oracle_id.clone(),
            feed_ids: vec!["BTC".into()],
        })
        .await?;
    assert_eq!(prices.entries.len(), 1);

    let set_role = stack
        .controller
        .request::<redstone::SetRole>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: redstone::SetRole {
                oracle_id: oracle_id.clone(),
                account_id: stack.harness.beneficiary_account_id.clone(),
                role: redstone::RoleValue::TrustedUpdater,
                set: true,
            },
        })
        .await?;
    assert_eq!(
        set_role.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let roles = stack
        .controller
        .request::<redstone::ListRole>(&redstone::ListRole {
            oracle_id: oracle_id.clone(),
            role: redstone::RoleValue::TrustedUpdater,
        })
        .await?;
    assert_eq!(
        roles.account_ids,
        vec![stack.harness.beneficiary_account_id.clone()]
    );

    let write = stack
        .controller
        .request::<redstone::WritePrices>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: redstone::WritePrices {
                oracle_id: oracle_id.clone(),
                feed_ids: vec!["ETH".into()],
                payload: Base64Bytes(vec![1, 2, 3]),
            },
        })
        .await?;
    assert_eq!(
        write.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let written = stack
        .controller
        .request::<redstone::ReadPriceData>(&redstone::ReadPriceData {
            oracle_id,
            feed_ids: vec!["ETH".into()],
        })
        .await?;
    assert_eq!(written.entries.len(), 1);
    assert_ne!(written.entries[0].data.price, U256::zero().into());

    stack.shutdown().await;
    Ok(())
}

/// `redstone.create` deploys from a registry with typed init args, so the adapter
/// must come up configured — proving the args this build serializes are the args
/// the contract's `new` accepts.
#[tokio::test]
async fn redstone_create_deploys_a_configured_adapter() -> Result<()> {
    let stack = TestStack::start().await?;
    let registry_id = stack.harness.deploy_registry().await?;

    stack
        .controller
        .request::<registry::AddVersion>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: registry::AddVersion {
                registry_id: registry_id.clone(),
                version_key: "redstone@0.2.0".to_owned(),
                deploy_mode: templar_common::registry::DeployMode::Normal,
                code: Base64Bytes(
                    templar_gateway_testing::wasm::redstone_adapter()
                        .await
                        .to_vec(),
                ),
                deposit: NearToken::from_yoctonear(1),
            },
        })
        .await?;

    let expected = templar_common::oracle::redstone::config::test();
    let create = stack
        .controller
        .request::<redstone::Create>(&WriteRequest {
            signer_account_id: stack.harness.registry_signer_account_id.clone(),
            idempotency_key: None,
            body: redstone::Create {
                target: registry::DeployTarget {
                    registry_id: registry_id.clone(),
                    name: "redstone-created".to_owned(),
                    version_key: "redstone@0.2.0".to_owned(),
                    full_access_keys: None,
                    deposit: NearToken::from_near(10),
                },
                config: expected.clone(),
                admin_id: stack.harness.beneficiary_account_id.clone(),
            },
        })
        .await?;
    assert_eq!(
        create.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let oracle_id = registry_id
        .sub_account("redstone-created")
        .expect("created adapter id should be valid");
    let config = stack
        .controller
        .request::<redstone::GetConfig>(&redstone::GetConfig {
            oracle_id: oracle_id.clone(),
        })
        .await?;
    assert_eq!(
        config.config.signer_count_threshold,
        expected.signer_count_threshold
    );
    assert_eq!(config.config.signers, expected.signers);

    // The `admin_id` the init args named, not the deploying registry.
    let admins = stack
        .controller
        .request::<redstone::ListRole>(&redstone::ListRole {
            oracle_id,
            role: redstone::RoleValue::ModifyRoles,
        })
        .await?;
    assert!(
        admins
            .account_ids
            .contains(&stack.harness.beneficiary_account_id),
        "{admins:?}"
    );

    stack.shutdown().await;
    Ok(())
}
