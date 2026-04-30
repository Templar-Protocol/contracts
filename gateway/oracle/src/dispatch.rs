use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::number::Decimal;
use templar_common::oracle::{
    price_transformer,
    proxy::{self, Source},
    pyth::{self, PriceIdentifier},
    redstone, OracleRequest,
};
use templar_common::time::Nanoseconds;
use templar_gateway_core::{
    client::{
        lst_oracle::GetTransformerArgs, proxy_oracle::GetProxyArgs,
        pyth_oracle::ListEmaPricesNoOlderThanArgs, redstone_oracle::ReadPriceDataArgs,
    },
    plan_pyth_update, plan_redstone_write_prices, query_contract_kind, DispatchRead, GatewayError,
    GatewayResult, HasNearClient, OperationPlan, OraclePayloadSource, PlanWrite,
};
use templar_gateway_types::contract::ContractKind;
use templar_gateway_types::oracle::{
    self, GetPriceResolutionDependenciesResult, OracleContractKind, RedStoneOraclePrices,
    RedStonePriceEntry, ResolvePricesResult, ResolvedPrice,
};
use templar_gateway_types::MethodSpec;

use crate::{ProvidesPythSource, ProvidesRedStoneSource};

pub struct Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<oracle::GetPriceResolutionDependencies, C> for Dispatch {
    async fn dispatch(
        request: <oracle::GetPriceResolutionDependencies as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<GetPriceResolutionDependenciesResult> {
        let params = request.params;
        let kind = query_oracle_kind(&ctx, params.oracle_id.clone()).await?;
        let requests = resolve_dependencies(&ctx, params.oracle_id, params.price_id, &kind).await?;
        Ok(GetPriceResolutionDependenciesResult { kind, requests })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<oracle::ResolvePrice, C> for Dispatch {
    async fn dispatch(
        request: <oracle::ResolvePrice as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<oracle::ResolvePriceResult> {
        let params = request.params;
        let inputs = ResolutionInputs::new(params.pyth, params.redstone);
        let price = resolve_price(
            &ctx,
            &inputs,
            params.oracle_id,
            params.price_id,
            Nanoseconds::from_secs(params.age),
        )
        .await?;
        Ok(oracle::ResolvePriceResult { price })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<oracle::ResolvePrices, C> for Dispatch {
    async fn dispatch(
        request: <oracle::ResolvePrices as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<ResolvePricesResult> {
        let params = request.params;
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
impl<C: HasNearClient> DispatchRead<oracle::GetPrice, C> for Dispatch {
    async fn dispatch(
        request: <oracle::GetPrice as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<<oracle::GetPrice as MethodSpec>::Output> {
        let params = request.params;
        let price = get_price_onchain(
            &ctx,
            params.oracle_id,
            params.price_id,
            Nanoseconds::from_secs(params.age),
        )
        .await?;
        Ok(oracle::GetPriceResult { price })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<oracle::GetPrices, C> for Dispatch {
    async fn dispatch(
        request: <oracle::GetPrices as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<ResolvePricesResult> {
        let params = request.params;
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

#[async_trait]
impl<C> PlanWrite<oracle::UpdatePyth, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource,
{
    async fn plan(
        request: <oracle::UpdatePyth as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        plan_pyth_update(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            body.vaa.0,
        )
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<oracle::UpdateRedStone, C> for Dispatch
where
    C: HasNearClient + ProvidesRedStoneSource,
{
    async fn plan(
        request: <oracle::UpdateRedStone as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let feed_id = body.feed_id;
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &[feed_id.clone()])
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        plan_redstone_write_prices(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            vec![feed_id],
            payload,
        )
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<oracle::UpdatePrices, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource,
{
    async fn plan(
        request: <oracle::UpdatePrices as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let requests =
            resolve_update_requests(&ctx, request.body.oracle_id, request.body.price_ids).await?;

        let mut steps = Vec::new();
        let mut pyth_updates = BTreeMap::<AccountId, BTreeSet<PriceIdentifier>>::new();
        let mut redstone_updates = BTreeMap::<AccountId, BTreeSet<redstone::FeedId>>::new();

        for request in requests {
            match request {
                OracleRequest::Pyth(request) => {
                    pyth_updates
                        .entry(request.oracle_id)
                        .or_default()
                        .insert(request.price_id);
                }
                OracleRequest::RedStone(request) => {
                    redstone_updates
                        .entry(request.oracle_id)
                        .or_default()
                        .insert(request.price_id);
                }
            }
        }

        for (oracle_id, price_ids) in pyth_updates {
            let price_ids = price_ids.into_iter().collect::<Vec<_>>();
            let vaa = OraclePayloadSource::fetch_payload(ctx.pyth_source(), &price_ids)
                .await
                .map_err(|error| GatewayError::HttpRequest(error.to_string()))?;
            steps.push(plan_pyth_update(
                ctx.near_client(),
                request.signer_account_id.clone(),
                oracle_id,
                vaa,
            )?);
        }

        for (oracle_id, feed_ids) in redstone_updates {
            let feed_ids = feed_ids.into_iter().collect::<Vec<_>>();
            let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &feed_ids)
                .await
                .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
            steps.push(plan_redstone_write_prices(
                ctx.near_client(),
                request.signer_account_id.clone(),
                oracle_id,
                feed_ids,
                payload,
            )?);
        }

        Ok(OperationPlan { steps })
    }
}

struct ResolutionInputs {
    pyth: HashMap<AccountId, pyth::OracleResponse>,
    redstone: HashMap<AccountId, HashMap<redstone::FeedId, redstone::FeedData>>,
}

impl ResolutionInputs {
    fn new(
        pyth_inputs: Vec<oracle::PythOraclePrices>,
        redstone_inputs: Vec<RedStoneOraclePrices>,
    ) -> Self {
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
                            .map(|item: RedStonePriceEntry| (item.feed_id, item.data))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}

async fn resolve_update_requests<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_ids: Vec<PriceIdentifier>,
) -> GatewayResult<Vec<OracleRequest>> {
    let kind = query_oracle_kind(ctx, oracle_id.clone()).await?;
    let mut requests = BTreeSet::new();

    for price_id in price_ids {
        requests.extend(resolve_dependencies(ctx, oracle_id.clone(), price_id, &kind).await?);
    }

    Ok(requests.into_iter().collect())
}

async fn get_proxy<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<proxy::Proxy>> {
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
                .entries
                .into_iter()
                .map(|entry| match entry.source {
                    Source::Request(request) => request,
                    Source::Transformer(transformer) => transformer.request,
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
            for entry in &proxy.entries {
                if let Some(price) = resolve_proxy_entry_price(ctx, inputs, entry, max_age).await? {
                    prices.push((price, entry.weight));
                }
            }
            Ok(proxy
                .aggregator
                .aggregate(&prices, system_time())
                .map(Into::into))
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
            for entry in &proxy.entries {
                if let Some(price) = resolve_proxy_entry_price_onchain(ctx, entry, max_age).await? {
                    prices.push((price, entry.weight));
                }
            }
            Ok(proxy
                .aggregator
                .aggregate(&prices, system_time())
                .map(Into::into))
        }
    }
}

async fn resolve_proxy_entry_price<C: HasNearClient>(
    ctx: &C,
    inputs: &ResolutionInputs,
    entry: &proxy::Entry,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    match &entry.source {
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
    entry: &proxy::Entry,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    match &entry.source {
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
    let publish_time = Nanoseconds::try_from_pyth(price.publish_time)?;
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
