use near_account_id::AccountId;
use templar_gateway_types::{common::Pagination, contract::ContractKind};
use templar_universal_account::authentication::ed25519;

use crate::{
    client::{
        cache::load_cached, lst_oracle::ListTransformersArgs, proxy_oracle::ListProxiesArgs,
        pyth_lazer_oracle::GetFeedsDataArgs, pyth_oracle::ListEmaPricesUnsafeArgs,
        universal_account::UaGetKeyArgs,
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
    if try_vault_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::Vault);
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
    if try_proxy_governance_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::ProxyGovernance);
    }
    if try_lst_oracle_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::LstOracle);
    }
    // Before RedStone and Pyth: a Pyth Lazer adapter answers RedStone's `get_config`
    // probe by name (deserialization would then error, not report MethodNotFound), so
    // it must be distinguished first — via serving the feed-id-native `get_feeds_data`
    // while (unlike a classic Pyth oracle) not serving the `PriceIdentifier` views.
    if try_pyth_lazer_oracle_kind(ctx, contract_id.clone()).await? {
        return Ok(ContractKind::PythLazerOracle);
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
            .registry(contract_id)
            .list_versions(Pagination::default())
            .await,
    )
}

async fn try_market_kind<C: HasNearClient>(ctx: &C, contract_id: AccountId) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .market(contract_id)
            .get_configuration(())
            .await,
    )
}

async fn try_vault_kind<C: HasNearClient>(ctx: &C, contract_id: AccountId) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .vault(contract_id)
            .get_idle_balance(())
            .await,
    )
}

async fn try_universal_account_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .universal_account(contract_id)
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

async fn try_proxy_governance_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    probe_kind(
        ctx.near_client()
            .proxy_governance(contract_id)
            .next_proposal_id(())
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

async fn try_pyth_lazer_oracle_kind<C: HasNearClient>(
    ctx: &C,
    contract_id: AccountId,
) -> GatewayResult<bool> {
    // The Pyth Lazer adapter serves the feed-id-native `get_feeds_data` and, unlike a classic
    // Pyth oracle, does NOT serve the `PriceIdentifier`-keyed `list_ema_prices_unsafe` (dropped
    // in ENG-434). Requiring both — serves `get_feeds_data`, lacks the classic view — pins it to
    // the real adapter and away from a plain Pyth oracle (or a kitchen-sink test mock that
    // implements every oracle interface). An empty feed-id set still returns `Ok({})`, so no feed
    // needs to exist to identify the adapter.
    let serves_feeds_data = probe_kind(
        ctx.near_client()
            .pyth_lazer_oracle(contract_id.clone())
            .get_feeds_data(GetFeedsDataArgs { feed_ids: vec![] })
            .await,
    )?;
    if !serves_feeds_data {
        return Ok(false);
    }
    let serves_classic_pyth_view = probe_kind(
        ctx.near_client()
            .pyth_oracle(contract_id)
            .list_ema_prices_unsafe(ListEmaPricesUnsafeArgs { price_ids: vec![] })
            .await,
    )?;
    Ok(!serves_classic_pyth_view)
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
