use super::*;

#[tokio::test]
async fn ref_finance_get_pools_endpoint_works_against_sandbox() -> Result<()> {
    let stack = TestStack::start().await?;
    let exchange_id = stack
        .harness
        .deploy_ref_finance(
            "ref.near".parse()?,
            vec![test_utils::controller::ref_finance::PoolInfo {
                token_account_ids: vec![
                    stack.harness.ft_contract_id.clone(),
                    stack.harness.beneficiary_account_id.clone(),
                ],
                shares_total_supply: near_sdk::json_types::U128(99),
            }],
        )
        .await?;

    let pools = stack
        .controller
        .request::<ref_finance::GetPools>(&ReadRequest {
            params: ref_finance::GetPoolsParams {
                exchange_id,
                from_index: Some(0),
                limit: Some(10),
            },
        })
        .await?;

    assert_eq!(pools.pools.len(), 1);
    assert_eq!(
        pools.pools[0].shares_total_supply,
        templar_gateway_types::U128(99)
    );

    stack.shutdown().await;
    Ok(())
}
