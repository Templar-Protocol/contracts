use near_account_id::AccountId;
use templar_gateway_types::ContractKind;

use super::*;

#[tokio::test]
async fn contract_get_kind_endpoint_identifies_protocol_contracts() -> Result<()> {
    let stack = TestStack::start().await?;

    let registry_id = stack.harness.deploy_registry().await?;
    let (market_id, _) = stack.harness.deploy_market().await?;
    let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;
    let proxy_governance_id = stack
        .harness
        .deploy_governance_contract(
            proxy_oracle_id.clone(),
            stack.harness.proxy_oracle_signer_account_id.0.clone(),
        )
        .await?;
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
        kind_of(&stack, registry_id.clone()).await?,
        ContractKind::Registry
    );
    assert_eq!(
        kind_of(&stack, market_id.clone()).await?,
        ContractKind::Market
    );
    assert_eq!(
        kind_of(&stack, proxy_oracle_id).await?,
        ContractKind::ProxyOracle
    );
    assert_eq!(
        kind_of(&stack, proxy_governance_id).await?,
        ContractKind::ProxyGovernance
    );
    assert_eq!(
        kind_of(&stack, lst_oracle_id).await?,
        ContractKind::LstOracle
    );
    assert_eq!(
        kind_of(&stack, universal_account_id).await?,
        ContractKind::UniversalAccount
    );
    assert_eq!(
        kind_of(&stack, redstone_oracle_id).await?,
        ContractKind::RedstoneOracle
    );
    assert_eq!(
        kind_of(&stack, stack.harness.ft_contract_id.clone()).await?,
        ContractKind::Unknown
    );

    stack.shutdown().await;
    Ok(())
}

async fn kind_of(stack: &TestStack, contract_id: AccountId) -> Result<ContractKind> {
    Ok(stack
        .controller
        .request::<contract::GetKind>(&contract::GetKind { contract_id })
        .await?
        .kind)
}
