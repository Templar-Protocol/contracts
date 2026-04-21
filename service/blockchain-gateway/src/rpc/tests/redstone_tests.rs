use super::*;

#[tokio::test]
async fn redstone_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let oracle_id = stack
        .harness
        .deploy_mock_oracle("redstone-low-level.near".parse()?)
        .await?;

    stack
        .harness
        .set_mock_oracle_redstone_price(oracle_id.clone(), "BTC".into(), Some(redstone_price(42.0)))
        .await?;

    let config = stack
        .controller
        .request::<redstone::GetConfig>(&ReadRequest {
            params: redstone::GetConfigParams {
                oracle_id: oracle_id.clone(),
            },
        })
        .await?;
    assert!(config.config.signer_count_threshold > 0);

    let prices = stack
        .controller
        .request::<redstone::ReadPriceData>(&ReadRequest {
            params: redstone::ReadPriceDataParams {
                oracle_id: oracle_id.clone(),
                feed_ids: vec!["BTC".into()],
            },
        })
        .await?;
    assert_eq!(prices.entries.len(), 1);

    let set_role = stack
        .controller
        .request::<redstone::SetRole>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: redstone::SetRoleBody {
                oracle_id: oracle_id.clone(),
                account_id: stack.harness.beneficiary_account_id.clone(),
                role: redstone::RoleValue::TrustedUpdater,
                set: true,
            },
        })
        .await?;
    assert_eq!(
        set_role.operation.status,
        blockchain_gateway_core::OperationStatus::Succeeded
    );

    let roles = stack
        .controller
        .request::<redstone::ListRole>(&ReadRequest {
            params: redstone::ListRoleParams {
                oracle_id: oracle_id.clone(),
                role: redstone::RoleValue::TrustedUpdater,
            },
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
            body: redstone::WritePricesBody {
                oracle_id: oracle_id.clone(),
                feed_ids: vec!["ETH".into()],
                payload: Base64Bytes(vec![1, 2, 3]),
            },
        })
        .await?;
    assert_eq!(
        write.operation.status,
        blockchain_gateway_core::OperationStatus::Succeeded
    );

    let written = stack
        .controller
        .request::<redstone::ReadPriceData>(&ReadRequest {
            params: redstone::ReadPriceDataParams {
                oracle_id,
                feed_ids: vec!["ETH".into()],
            },
        })
        .await?;
    assert_eq!(written.entries.len(), 1);
    assert_ne!(written.entries[0].data.price, U256::zero().into());

    stack.shutdown().await;
    Ok(())
}
