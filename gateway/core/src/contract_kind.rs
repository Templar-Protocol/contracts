use templar_gateway_types::contract::ContractKind;

use crate::{client::cache::load_cached, GatewayError, GatewayResult, HasNearClient};

pub async fn query_contract_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<ContractKind> {
    load_cached(
        &ctx.near_client().cache().contract.contract_kind,
        contract_id.clone(),
        {
            let ctx = ctx.clone();
            move || async move { detect_contract_kind(&ctx, contract_id).await }
        },
    )
    .await
}

async fn detect_contract_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<ContractKind> {
    if matches!(
        try_registry_kind(ctx, contract_id.clone()).await?,
        Some(ContractKind::Registry)
    ) {
        return Ok(ContractKind::Registry);
    }
    if matches!(
        try_market_kind(ctx, contract_id.clone()).await?,
        Some(ContractKind::Market)
    ) {
        return Ok(ContractKind::Market);
    }
    if matches!(
        try_universal_account_kind(ctx, contract_id.clone()).await?,
        Some(ContractKind::UniversalAccount)
    ) {
        return Ok(ContractKind::UniversalAccount);
    }
    if matches!(
        try_proxy_oracle_kind(ctx, contract_id.clone()).await?,
        Some(ContractKind::ProxyOracle)
    ) {
        return Ok(ContractKind::ProxyOracle);
    }
    if matches!(
        try_lst_oracle_kind(ctx, contract_id.clone()).await?,
        Some(ContractKind::LstOracle)
    ) {
        return Ok(ContractKind::LstOracle);
    }
    if matches!(
        try_redstone_oracle_kind(ctx, contract_id.clone()).await?,
        Some(ContractKind::RedstoneOracle)
    ) {
        return Ok(ContractKind::RedstoneOracle);
    }
    if matches!(
        try_pyth_oracle_kind(ctx, contract_id).await?,
        Some(ContractKind::PythOracle)
    ) {
        return Ok(ContractKind::PythOracle);
    }
    Ok(ContractKind::Unknown)
}

async fn try_registry_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .registry(templar_gateway_types::RegistryId(contract_id))
        .list_versions(templar_gateway_types::common::Pagination::default())
        .await
    {
        Ok(_) => Ok(Some(ContractKind::Registry)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_market_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .market(templar_gateway_types::MarketId(contract_id))
        .get_configuration(())
        .await
    {
        Ok(_) => Ok(Some(ContractKind::Market)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_universal_account_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .universal_account(templar_gateway_types::UniversalAccountId(contract_id))
        .get_key(crate::client::universal_account::UaGetKeyArgs {
            key: templar_universal_account::KeyId::Ed25519Raw(
                templar_universal_account::authentication::ed25519::raw::VerifyKey(
                    [0_u8; 32].into(),
                ),
            ),
        })
        .await
    {
        Ok(_) => Ok(Some(ContractKind::UniversalAccount)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_proxy_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .proxy_oracle(contract_id)
        .list_proxies(crate::client::proxy_oracle::ListProxiesArgs {
            offset: None,
            count: Some(1),
        })
        .await
    {
        Ok(_) => Ok(Some(ContractKind::ProxyOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_lst_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .lst_oracle(contract_id)
        .list_transformers(crate::client::lst_oracle::ListTransformersArgs {
            offset: None,
            count: Some(1),
        })
        .await
    {
        Ok(_) => Ok(Some(ContractKind::LstOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_redstone_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .redstone_oracle(contract_id)
        .get_config(())
        .await
    {
        Ok(_) => Ok(Some(ContractKind::RedstoneOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_pyth_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<ContractKind>> {
    match ctx
        .near_client()
        .pyth_oracle(contract_id)
        .list_ema_prices_unsafe(crate::client::pyth_oracle::ListEmaPricesUnsafeArgs {
            price_ids: vec![],
        })
        .await
    {
        Ok(_) => Ok(Some(ContractKind::PythOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_method_not_found(error: &GatewayError) -> bool {
    matches!(error, GatewayError::NearQuery(message) if message.contains("MethodNotFound"))
}
