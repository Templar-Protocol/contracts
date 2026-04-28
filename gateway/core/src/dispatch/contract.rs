use futures::future::BoxFuture;
use templar_gateway_types::contract;

use crate::DispatchRead;
use crate::{client::cache::load_cached, GatewayContext, GatewayError, GatewayResult};

impl DispatchRead<GatewayContext> for contract::ViewFunction {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let value = ctx
                .near()
                .contract(request.params.contract_id.clone())
                .view_function(
                    &request.params.method_name.0,
                    request.params.args.try_into_bytes()?,
                )
                .await?;

            Ok(contract::ViewFunctionResult { value })
        })
    }
}

impl DispatchRead<GatewayContext> for contract::GetVersion {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let metadata = ctx
                .near()
                .contract(request.params.contract_id)
                .cached_contract_source_metadata()
                .await?;
            let version_string = metadata.version.ok_or_else(|| {
                crate::GatewayError::NearQuery(
                    "contract metadata does not contain version".to_owned(),
                )
            })?;

            Ok(contract::VersionResult {
                parsed: version_string.parse().ok(),
                version_string,
            })
        })
    }
}

impl DispatchRead<GatewayContext> for contract::GetKind {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let kind = query_contract_kind(&ctx, request.params.contract_id).await?;
            Ok(contract::GetKindResult { kind })
        })
    }
}

pub(crate) async fn query_contract_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<contract::ContractKind> {
    load_cached(
        &ctx.near().cache().contract.contract_kind,
        contract_id.clone(),
        {
            let ctx = ctx.clone();
            move || async move { detect_contract_kind(&ctx, contract_id).await }
        },
    )
    .await
}

async fn detect_contract_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<contract::ContractKind> {
    if matches!(
        try_registry_kind(ctx, contract_id.clone()).await?,
        Some(contract::ContractKind::Registry)
    ) {
        return Ok(contract::ContractKind::Registry);
    }

    if matches!(
        try_market_kind(ctx, contract_id.clone()).await?,
        Some(contract::ContractKind::Market)
    ) {
        return Ok(contract::ContractKind::Market);
    }

    if matches!(
        try_universal_account_kind(ctx, contract_id.clone()).await?,
        Some(contract::ContractKind::UniversalAccount)
    ) {
        return Ok(contract::ContractKind::UniversalAccount);
    }

    if matches!(
        try_proxy_oracle_kind(ctx, contract_id.clone()).await?,
        Some(contract::ContractKind::ProxyOracle)
    ) {
        return Ok(contract::ContractKind::ProxyOracle);
    }

    if matches!(
        try_lst_oracle_kind(ctx, contract_id.clone()).await?,
        Some(contract::ContractKind::LstOracle)
    ) {
        return Ok(contract::ContractKind::LstOracle);
    }

    if matches!(
        try_redstone_oracle_kind(ctx, contract_id.clone()).await?,
        Some(contract::ContractKind::RedstoneOracle)
    ) {
        return Ok(contract::ContractKind::RedstoneOracle);
    }

    if matches!(
        try_pyth_oracle_kind(ctx, contract_id).await?,
        Some(contract::ContractKind::PythOracle)
    ) {
        return Ok(contract::ContractKind::PythOracle);
    }

    Ok(contract::ContractKind::Unknown)
}

async fn try_registry_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx
        .near()
        .registry(templar_gateway_types::RegistryId(contract_id))
        .list_versions(templar_gateway_types::common::Pagination::default())
        .await
    {
        Ok(_) => Ok(Some(contract::ContractKind::Registry)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_market_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx
        .near()
        .market(templar_gateway_types::MarketId(contract_id))
        .get_configuration(())
        .await
    {
        Ok(_) => Ok(Some(contract::ContractKind::Market)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_universal_account_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx
        .near()
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
        Ok(_) => Ok(Some(contract::ContractKind::UniversalAccount)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_proxy_oracle_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx
        .near()
        .proxy_oracle(contract_id)
        .list_proxies(crate::client::proxy_oracle::ListProxiesArgs {
            offset: None,
            count: Some(1),
        })
        .await
    {
        Ok(_) => Ok(Some(contract::ContractKind::ProxyOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_lst_oracle_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx
        .near()
        .lst_oracle(contract_id)
        .list_transformers(crate::client::lst_oracle::ListTransformersArgs {
            offset: None,
            count: Some(1),
        })
        .await
    {
        Ok(_) => Ok(Some(contract::ContractKind::LstOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_redstone_oracle_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx.near().redstone_oracle(contract_id).get_config(()).await {
        Ok(_) => Ok(Some(contract::ContractKind::RedstoneOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_pyth_oracle_kind(
    ctx: &GatewayContext,
    contract_id: near_account_id::AccountId,
) -> GatewayResult<Option<contract::ContractKind>> {
    match ctx
        .near()
        .pyth_oracle(contract_id)
        .list_ema_prices_unsafe(crate::client::pyth_oracle::ListEmaPricesUnsafeArgs {
            price_ids: vec![],
        })
        .await
    {
        Ok(_) => Ok(Some(contract::ContractKind::PythOracle)),
        Err(error) if is_method_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_method_not_found(error: &GatewayError) -> bool {
    matches!(error, GatewayError::NearQuery(message) if message.contains("MethodNotFound"))
}
