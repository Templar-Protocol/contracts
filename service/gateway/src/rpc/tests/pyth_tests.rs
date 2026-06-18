use super::*;

#[tokio::test]
async fn pyth_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let oracle_id = stack
        .harness
        .deploy_mock_oracle("pyth-low-level.near".parse()?)
        .await?;
    let price_id = test_utils::DEFAULT_BORROW_PRICE_ID;
    let price = pyth_price(123.45);
    stack
        .harness
        .set_mock_oracle_pyth_price(oracle_id.clone(), price_id, Some(price.clone()))
        .await?;

    let unsafe_prices = stack
        .controller
        .request::<pyth::ListEmaPricesUnsafe>(&ReadRequest {
            params: pyth::ListEmaPricesUnsafeParams {
                oracle_id: oracle_id.clone(),
                price_ids: vec![price_id],
            },
        })
        .await?;
    assert_same_pyth_price_value(unsafe_prices.prices[0].price.clone(), &price);

    let bounded_prices = stack
        .controller
        .request::<pyth::ListEmaPricesNoOlderThan>(&ReadRequest {
            params: pyth::ListEmaPricesNoOlderThanParams {
                oracle_id: oracle_id.clone(),
                price_ids: vec![price_id],
                age: 60,
            },
        })
        .await?;
    assert_same_pyth_price_value(bounded_prices.prices[0].price.clone(), &price);

    let update = stack
        .controller
        .request::<pyth::UpdatePriceFeeds>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: pyth::UpdatePriceFeedsBody {
                oracle_id: oracle_id.clone(),
                data: Base64Bytes(vec![0xca, 0xfe, 0xba, 0xbe]),
            },
        })
        .await?;
    assert_eq!(
        update.operation.status,
        templar_gateway_types::OperationStatus::Succeeded
    );

    let last_update = view_contract_json(
        &stack,
        oracle_id,
        "last_pyth_update_data",
        serde_json::json!({}),
    )
    .await?;
    assert_eq!(last_update, serde_json::json!("cafebabe"));

    stack.shutdown().await;
    Ok(())
}
