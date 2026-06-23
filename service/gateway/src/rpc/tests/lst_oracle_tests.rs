use super::*;

#[tokio::test]
async fn lst_oracle_endpoints_work_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let pyth_oracle_id = stack
        .harness
        .deploy_mock_oracle("lst-pyth.near".parse()?)
        .await?;
    let lst_oracle_id = stack
        .harness
        .deploy_lst_oracle("lst-oracle.near".parse()?, pyth_oracle_id.clone())
        .await?;

    let transformed_price_id = PriceIdentifier([0x42; 32]);
    let transformer = PriceTransformer::lst(
        test_utils::DEFAULT_BORROW_PRICE_ID,
        24,
        price_transformer::Call {
            account_id: stack.harness.ft_contract_id.clone(),
            method_name: "redemption_rate".to_owned(),
            args: near_sdk::json_types::Base64VecU8(serde_json::to_vec(&serde_json::Value::Null)?),
            gas: near_sdk::json_types::U64(near_sdk::Gas::from_tgas(3).as_gas()),
        },
    );
    stack
        .harness
        .create_lst_transformer(
            lst_oracle_id.clone(),
            transformed_price_id,
            transformer.clone(),
        )
        .await?;

    let get_oracle_id = stack
        .controller
        .request::<lst_oracle::GetOracleId>(&lst_oracle::GetOracleId {
            oracle_id: lst_oracle_id.clone(),
        })
        .await?;
    assert_eq!(get_oracle_id.pyth_oracle_id, pyth_oracle_id);

    let list = stack
        .controller
        .request::<lst_oracle::ListTransformers>(&lst_oracle::ListTransformers {
            oracle_id: lst_oracle_id.clone(),
            pagination: templar_gateway_types::common::Pagination::default(),
        })
        .await?;
    assert_eq!(list.price_ids, vec![transformed_price_id]);

    let get = stack
        .controller
        .request::<lst_oracle::GetTransformer>(&lst_oracle::GetTransformer {
            oracle_id: lst_oracle_id,
            price_identifier: transformed_price_id,
        })
        .await?;
    assert_eq!(get.transformer, Some(transformer));

    stack.shutdown().await;
    Ok(())
}
