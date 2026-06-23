use std::collections::{BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::oracle::{
    pyth::{self, PriceIdentifier},
    redstone,
};
use templar_common::{Decimal, Nanoseconds};
use templar_gateway_core::{
    client::{
        lst_oracle::GetTransformerArgs, proxy_oracle::GetProxyArgs,
        pyth_oracle::ListEmaPricesNoOlderThanArgs, redstone_oracle::ReadPriceDataArgs,
    },
    query_contract_kind, DispatchRead, GatewayError, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::oracle::{
    GetPrice, GetPriceResolutionDependencies, GetPriceResolutionDependenciesResult, GetPriceResult,
    GetPrices, OracleContractKind, PythOraclePrices, RedStoneOraclePrices, ResolvePrice,
    ResolvePriceResult, ResolvePrices, ResolvePricesResult, ResolvedPrice,
};
use templar_gateway_types::contract::ContractKind;
use templar_proxy_oracle_kernel::proxy;
use templar_proxy_oracle_kernel::proxy::aggregator::method::Aggregate;
use templar_proxy_oracle_near_common::convert;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_common::price_transformer;
use templar_proxy_oracle_near_common::request::OracleRequest;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<GetPriceResolutionDependencies, C> for Dispatch {
    async fn dispatch(
        request: GetPriceResolutionDependencies,
        ctx: C,
    ) -> GatewayResult<GetPriceResolutionDependenciesResult> {
        let params = request;
        let kind = query_oracle_kind(&ctx, params.oracle_id.clone()).await?;
        let requests = resolve_dependencies(&ctx, params.oracle_id, params.price_id, &kind).await?;
        Ok(GetPriceResolutionDependenciesResult { kind, requests })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<ResolvePrice, C> for Dispatch {
    async fn dispatch(request: ResolvePrice, ctx: C) -> GatewayResult<ResolvePriceResult> {
        let params = request;
        let inputs = ResolutionInputs::new(params.pyth, params.redstone);
        let price = resolve_price(
            &ctx,
            &inputs,
            params.oracle_id,
            params.price_id,
            Nanoseconds::from_secs(params.age),
        )
        .await?;
        Ok(ResolvePriceResult { price })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<ResolvePrices, C> for Dispatch {
    async fn dispatch(request: ResolvePrices, ctx: C) -> GatewayResult<ResolvePricesResult> {
        let params = request;
        let inputs = ResolutionInputs::new(params.pyth, params.redstone);
        let max_age = Nanoseconds::from_secs(params.age);
        let mut prices = Vec::with_capacity(params.price_ids.len());
        for price_id in params.price_ids {
            let price =
                resolve_price(&ctx, &inputs, params.oracle_id.clone(), price_id, max_age).await?;
            prices.push(ResolvedPrice { price_id, price });
        }
        Ok(ResolvePricesResult { prices })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<GetPrice, C> for Dispatch {
    async fn dispatch(request: GetPrice, ctx: C) -> GatewayResult<GetPriceResult> {
        let params = request;
        let price = get_price_onchain(
            &ctx,
            params.oracle_id,
            params.price_id,
            Nanoseconds::from_secs(params.age),
        )
        .await?;
        Ok(GetPriceResult { price })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<GetPrices, C> for Dispatch {
    async fn dispatch(request: GetPrices, ctx: C) -> GatewayResult<ResolvePricesResult> {
        let params = request;
        let max_age = Nanoseconds::from_secs(params.age);
        let mut prices = Vec::with_capacity(params.price_ids.len());
        for price_id in params.price_ids {
            let price =
                get_price_onchain(&ctx, params.oracle_id.clone(), price_id, max_age).await?;
            prices.push(ResolvedPrice { price_id, price });
        }
        Ok(ResolvePricesResult { prices })
    }
}

struct ResolutionInputs {
    pyth: HashMap<AccountId, pyth::OracleResponse>,
    redstone: HashMap<AccountId, HashMap<redstone::FeedId, redstone::FeedData>>,
}

impl ResolutionInputs {
    fn new(pyth_inputs: Vec<PythOraclePrices>, redstone_inputs: Vec<RedStoneOraclePrices>) -> Self {
        Self {
            pyth: pyth_inputs
                .into_iter()
                .map(|entry| (entry.oracle_id, entry.response))
                .collect(),
            redstone: redstone_inputs
                .into_iter()
                .map(|entry| {
                    (
                        entry.oracle_id,
                        entry
                            .response
                            .into_iter()
                            .map(|item| (item.feed_id, item.data))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}

async fn get_proxy<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<proxy::Proxy<Source>>> {
    ctx.near_client()
        .proxy_oracle(oracle_id)
        .cached_get_proxy(GetProxyArgs { id })
        .await
}

async fn query_oracle_kind<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
) -> GatewayResult<OracleContractKind> {
    match query_contract_kind(ctx, oracle_id.clone()).await? {
        ContractKind::PythOracle | ContractKind::RedstoneOracle => Ok(OracleContractKind::Direct),
        ContractKind::ProxyOracle => Ok(OracleContractKind::Proxy),
        ContractKind::LstOracle => {
            let pyth_id = ctx
                .near_client()
                .lst_oracle(oracle_id)
                .cached_oracle_id()
                .await?;
            Ok(OracleContractKind::Lst { pyth_id })
        }
        other => Err(GatewayError::NearQuery(format!(
            "contract kind {other:?} is not an oracle contract"
        ))),
    }
}

async fn resolve_dependencies<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    kind: &OracleContractKind,
) -> GatewayResult<Vec<OracleRequest>> {
    match kind.clone() {
        OracleContractKind::Direct => Ok(vec![OracleRequest::pyth(oracle_id, price_id)]),
        OracleContractKind::Lst { pyth_id } => {
            let transformer = ctx
                .near_client()
                .lst_oracle(oracle_id)
                .cached_get_transformer(GetTransformerArgs {
                    price_identifier: price_id,
                })
                .await?;
            Ok(vec![transformer.map_or_else(
                || OracleRequest::pyth(pyth_id.clone(), price_id),
                |transformer| OracleRequest::pyth(pyth_id.clone(), transformer.price_id),
            )])
        }
        OracleContractKind::Proxy => {
            let proxy = get_proxy(ctx, oracle_id, price_id).await?.ok_or_else(|| {
                GatewayError::NearQuery("price identifier not found on proxy oracle".to_owned())
            })?;
            let requests = proxy
                .sources()
                .map(|source| match source {
                    Source::Request(request) => request.clone(),
                    Source::Transformer(transformer) => transformer.request.clone(),
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if requests.is_empty() {
                return Err(GatewayError::NearQuery(
                    "proxy oracle returned empty proxy definition".to_owned(),
                ));
            }
            Ok(requests)
        }
    }
}

async fn resolve_price<C: HasNearClient>(
    ctx: &C,
    inputs: &ResolutionInputs,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let kind = query_oracle_kind(ctx, oracle_id.clone()).await?;
    match kind {
        OracleContractKind::Direct => Ok(fetch_oracle_request(
            inputs,
            OracleRequest::pyth(oracle_id, price_id),
            max_age,
        )),
        OracleContractKind::Lst { pyth_id } => {
            let transformer = ctx
                .near_client()
                .lst_oracle(oracle_id)
                .cached_get_transformer(GetTransformerArgs {
                    price_identifier: price_id,
                })
                .await?;
            match transformer {
                Some(transformer) => {
                    let Some(price) = fetch_oracle_request(
                        inputs,
                        OracleRequest::pyth(pyth_id, transformer.price_id),
                        max_age,
                    ) else {
                        return Ok(None);
                    };
                    let input = fetch_transformer_input(ctx, transformer.call).await?;
                    Ok(transformer.action.apply(price, input))
                }
                None => Ok(fetch_oracle_request(
                    inputs,
                    OracleRequest::pyth(pyth_id, price_id),
                    max_age,
                )),
            }
        }
        OracleContractKind::Proxy => {
            let proxy = get_proxy(ctx, oracle_id, price_id).await?.ok_or_else(|| {
                GatewayError::NearQuery("price identifier not found on proxy oracle".to_owned())
            })?;
            let mut prices = vec![];
            for source in proxy.sources() {
                let price = resolve_proxy_entry_price(ctx, inputs, source, max_age)
                    .await?
                    .as_ref()
                    .and_then(convert::pyth_price_try_to_kernel);
                prices.push(price);
            }
            Ok(proxy
                .aggregator
                .aggregate(prices)
                .ok()
                .as_ref()
                .and_then(convert::pyth_price_try_from_kernel))
        }
    }
}

async fn get_price_onchain<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let kind = query_oracle_kind(ctx, oracle_id.clone()).await?;
    match kind {
        OracleContractKind::Direct => {
            fetch_oracle_request_onchain(ctx, OracleRequest::pyth(oracle_id, price_id), max_age)
                .await
        }
        OracleContractKind::Lst { pyth_id } => {
            let transformer = ctx
                .near_client()
                .lst_oracle(oracle_id.clone())
                .cached_get_transformer(GetTransformerArgs {
                    price_identifier: price_id,
                })
                .await?;
            match transformer {
                Some(transformer) => {
                    let Some(price) = fetch_oracle_request_onchain(
                        ctx,
                        OracleRequest::pyth(pyth_id, transformer.price_id),
                        max_age,
                    )
                    .await?
                    else {
                        return Ok(None);
                    };
                    let input = fetch_transformer_input(ctx, transformer.call).await?;
                    Ok(transformer.action.apply(price, input))
                }
                None => {
                    fetch_oracle_request_onchain(
                        ctx,
                        OracleRequest::pyth(pyth_id, price_id),
                        max_age,
                    )
                    .await
                }
            }
        }
        OracleContractKind::Proxy => {
            let proxy = get_proxy(ctx, oracle_id, price_id).await?.ok_or_else(|| {
                GatewayError::NearQuery("price identifier not found on proxy oracle".to_owned())
            })?;
            let mut prices = vec![];
            for source in proxy.sources() {
                let price = resolve_proxy_entry_price_onchain(ctx, source, max_age)
                    .await?
                    .as_ref()
                    .and_then(convert::pyth_price_try_to_kernel);
                prices.push(price);
            }
            Ok(proxy
                .aggregator
                .aggregate(prices)
                .ok()
                .as_ref()
                .and_then(convert::pyth_price_try_from_kernel))
        }
    }
}

async fn resolve_proxy_entry_price<C: HasNearClient>(
    ctx: &C,
    inputs: &ResolutionInputs,
    source: &Source,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    match source {
        Source::Request(request) => Ok(fetch_oracle_request(inputs, request.clone(), max_age)),
        Source::Transformer(transformer) => {
            let Some(price) = fetch_oracle_request(inputs, transformer.request.clone(), max_age)
            else {
                return Ok(None);
            };
            let input = fetch_transformer_input(ctx, transformer.call.clone()).await?;
            Ok(transformer.action.apply(price, input))
        }
    }
}

async fn resolve_proxy_entry_price_onchain<C: HasNearClient>(
    ctx: &C,
    source: &Source,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    match source {
        Source::Request(request) => {
            fetch_oracle_request_onchain(ctx, request.clone(), max_age).await
        }
        Source::Transformer(transformer) => {
            let Some(price) =
                fetch_oracle_request_onchain(ctx, transformer.request.clone(), max_age).await?
            else {
                return Ok(None);
            };
            let input = fetch_transformer_input(ctx, transformer.call.clone()).await?;
            Ok(transformer.action.apply(price, input))
        }
    }
}

async fn fetch_transformer_input<C: HasNearClient>(
    ctx: &C,
    call: price_transformer::Call,
) -> GatewayResult<Decimal> {
    ctx.near_client()
        .contract(call.account_id)
        .view_function(&call.method_name, call.args.0)
        .await
}

fn fetch_oracle_request(
    inputs: &ResolutionInputs,
    request: OracleRequest,
    max_age: Nanoseconds,
) -> Option<pyth::Price> {
    let fetched_price = match request {
        OracleRequest::Pyth(request) => inputs
            .pyth
            .get(&request.oracle_id)
            .and_then(|response| response.get(&request.price_id))
            .cloned()
            .flatten(),
        OracleRequest::RedStone(request) => inputs
            .redstone
            .get(&request.oracle_id)
            .and_then(|response| response.get(&request.price_id))
            .cloned()
            .and_then(|feed| feed.to_pyth_price()),
    }?;
    validate_price_age(fetched_price, max_age)
}

async fn fetch_oracle_request_onchain<C: HasNearClient>(
    ctx: &C,
    request: OracleRequest,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let fetched_price = match request {
        OracleRequest::Pyth(request) => ctx
            .near_client()
            .pyth_oracle(request.oracle_id)
            .list_ema_prices_no_older_than(ListEmaPricesNoOlderThanArgs {
                price_ids: vec![request.price_id],
                age: max_age.as_secs(),
            })
            .await?
            .remove(&request.price_id)
            .flatten(),
        OracleRequest::RedStone(request) => ctx
            .near_client()
            .redstone_oracle(request.oracle_id)
            .read_price_data(ReadPriceDataArgs {
                feed_ids: vec![request.price_id.clone()],
            })
            .await?
            .remove(&request.price_id)
            .and_then(|feed| feed.to_pyth_price()),
    };
    Ok(fetched_price.and_then(|price| validate_price_age(price, max_age)))
}

fn validate_price_age(price: pyth::Price, max_age: Nanoseconds) -> Option<pyth::Price> {
    let publish_time = price.publish_time.try_into_time()?;
    let now = system_time();
    if now >= publish_time && now.saturating_sub(publish_time) > max_age {
        return None;
    }
    Some(price)
}

fn system_time() -> Nanoseconds {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Nanoseconds::from_ns(u64::try_from(now).unwrap_or(u64::MAX))
}
