use near_account_id::AccountId;
use templar_gateway_types::{
    common::Pagination, contract::ContractKind, MarketId, RegistryId, UniversalAccountId,
};
use templar_universal_account::authentication::ed25519;

use crate::{
    client::{
        cache::load_cached, lst_oracle::ListTransformersArgs, proxy_oracle::ListProxiesArgs,
        pyth_oracle::ListEmaPricesUnsafeArgs, universal_account::UaGetKeyArgs,
    },
    GatewayError, GatewayResult, HasNearClient,
};

pub async fn query_contract_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
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
    contract_id: AccountId,
) -> GatewayResult<ContractKind> {
    if try_registry_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::Registry);
    }
    if try_market_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::Market);
    }
    if try_universal_account_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::UniversalAccount);
    }
    if try_proxy_oracle_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::ProxyOracle);
    }
    if try_lst_oracle_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::LstOracle);
    }
    if try_redstone_oracle_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::RedstoneOracle);
    }
    if try_pyth_oracle_kind(ctx, contract_id).await? {
        return Ok(ContractKind::PythOracle);
    }
    Ok(ContractKind::Unknown)
}

async fn try_registry_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .registry(RegistryId(contract_id))
            .list_versions(Pagination::default())
            .await,
    )
}

async fn try_market_kind<C: HasNearClient>(ctx: &C, contract_id: AccountId) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .market(MarketId(contract_id))
            .get_configuration(())
            .await,
    )
}

async fn try_universal_account_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .universal_account(UniversalAccountId(contract_id))
            .get_key(UaGetKeyArgs {
                key: ed25519::raw::VerifyKey([0_u8; 32].into()).into(),
            })
            .await,
    )
}

async fn try_proxy_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .proxy_oracle(contract_id)
            .list_proxies(ListProxiesArgs {
                offset: None,
                count: Some(1),
            })
            .await,
    )
}

async fn try_lst_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .lst_oracle(contract_id)
            .list_transformers(ListTransformersArgs {
                offset: None,
                count: Some(1),
            })
            .await,
    )
}

async fn try_redstone_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .redstone_oracle(contract_id)
            .get_config(())
            .await,
    )
}

async fn try_pyth_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .pyth_oracle(contract_id)
            .list_ema_prices_unsafe(ListEmaPricesUnsafeArgs { price_ids: vec![] })
            .await,
    )
}

fn probe_kind<T>(result: GatewayResult<T>) -> GatewayResult<bool> {
    match result {
        Ok(_) => Ok(true),
        Err(error) if is_method_not_found(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_method_not_found(error: &GatewayError) -> bool {
    matches!(error, GatewayError::NearQuery(message) if message.contains("MethodNotFound"))
}
