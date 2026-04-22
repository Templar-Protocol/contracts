use super::*;

#[tokio::test]
async fn contract_get_kind_endpoint_identifies_protocol_contracts() -> Result<()> {
    let stack = TestStack::start().await?;

    let registry_id = stack.harness.deploy_registry().await?;
    let (market_id, _) = stack.harness.deploy_market().await?;
    let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;
    let pyth_oracle_id = stack
        .harness
        .deploy_mock_oracle("kind-pyth.near".parse()?)
        .await?;
    let lst_oracle_id = stack
        .harness
        .deploy_lst_oracle("kind-lst.near".parse()?, pyth_oracle_id.clone())
        .await?;
    let (universal_account_id, _) = stack.harness.deploy_universal_account().await?;
    let redstone_oracle_id = stack
        .harness
        .deploy_redstone_adapter("kind-redstone.near".parse()?)
        .await?;

    assert_eq!(
        kind_of(&stack, registry_id.0.clone()).await?,
        contract::ContractKind::Registry
    );
    assert_eq!(
        kind_of(&stack, market_id.0.clone()).await?,
        contract::ContractKind::Market
    );
    assert_eq!(
        kind_of(&stack, proxy_oracle_id).await?,
        contract::ContractKind::ProxyOracle
    );
    assert_eq!(
        kind_of(&stack, lst_oracle_id).await?,
        contract::ContractKind::LstOracle
    );
    assert_eq!(
        kind_of(&stack, universal_account_id.0).await?,
        contract::ContractKind::UniversalAccount
    );
    assert_eq!(
        kind_of(&stack, redstone_oracle_id).await?,
        contract::ContractKind::RedstoneOracle
    );
    assert_eq!(
        kind_of(&stack, stack.harness.ft_contract_id.clone()).await?,
        contract::ContractKind::Unknown
    );

    stack.shutdown().await;
    Ok(())
}

async fn kind_of(
    stack: &TestStack,
    contract_id: near_account_id::AccountId,
) -> Result<contract::ContractKind> {
    Ok(stack
        .controller
        .request::<contract::GetKind>(&ReadRequest {
            params: contract::GetKindParams { contract_id },
        })
        .await?
        .kind)
}
