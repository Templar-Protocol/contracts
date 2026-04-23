use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use near_account_id::AccountId;
use near_sdk::json_types::Base64VecU8;
use near_sdk::NearToken;
use templar_common::oracle::price_transformer;
use templar_common::{
    number::Decimal,
    oracle::{
        proxy::Source,
        pyth::{self, PriceIdentifier},
        redstone, OracleRequest,
    },
    time::Nanoseconds,
};
use templar_gateway_types::oracle::{
    self, GetPriceResolutionDependenciesResult, OracleContractKind, RedStoneOraclePrices,
    RedStonePriceEntry, ResolvePricesResult, ResolvedPrice,
};
use templar_gateway_types::OraclePayloadSource;

use crate::operation::PlannedTransaction;
use crate::{
    client::{
        lst_oracle::GetTransformerArgs,
        proxy_oracle::GetProxyArgs,
        pyth_oracle::{ListEmaPricesNoOlderThanArgs, UpdatePriceFeedsArgs},
        redstone_oracle::{ReadPriceDataArgs, WritePricesArgs},
        ContractWriteOptions,
    },
    dispatch::contract::query_contract_kind,
    GatewayContext, GatewayError, GatewayResult,
};
use crate::{DispatchRead, PlanWrite};

const PYTH_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

async fn get_proxy(
    ctx: &GatewayContext,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<templar_common::oracle::proxy::Proxy>> {
    ctx.proxy_oracle(oracle_id)
        .cached_get_proxy(GetProxyArgs { id })
        .await
}

impl DispatchRead<GatewayContext> for oracle::GetPriceResolutionDependencies {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let kind = query_oracle_kind(&ctx, params.oracle_id.clone()).await?;
            let requests =
                resolve_dependencies(&ctx, params.oracle_id, params.price_id, &kind).await?;
            Ok(GetPriceResolutionDependenciesResult { kind, requests })
        })
    }
}

impl DispatchRead<GatewayContext> for oracle::ResolvePrice {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
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
        })
    }
}

impl DispatchRead<GatewayContext> for oracle::ResolvePrices {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let inputs = ResolutionInputs::new(params.pyth, params.redstone);
            let max_age = Nanoseconds::from_secs(params.age);
            let mut prices = Vec::with_capacity(params.price_ids.len());
            for price_id in params.price_ids {
                let price =
                    resolve_price(&ctx, &inputs, params.oracle_id.clone(), price_id, max_age)
                        .await?;
                prices.push(ResolvedPrice { price_id, price });
            }
            Ok(ResolvePricesResult { prices })
        })
    }
}

impl DispatchRead<GatewayContext> for oracle::GetPrice {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let price = get_price_onchain(
                &ctx,
                params.oracle_id,
                params.price_id,
                Nanoseconds::from_secs(params.age),
            )
            .await?;
            Ok(oracle::GetPriceResult { price })
        })
    }
}

impl DispatchRead<GatewayContext> for oracle::GetPrices {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let max_age = Nanoseconds::from_secs(params.age);
            let mut prices = Vec::with_capacity(params.price_ids.len());
            for price_id in params.price_ids {
                let price =
                    get_price_onchain(&ctx, params.oracle_id.clone(), price_id, max_age).await?;
                prices.push(ResolvedPrice { price_id, price });
            }
            Ok(ResolvePricesResult { prices })
        })
    }
}

impl PlanWrite<GatewayContext> for oracle::UpdatePyth {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            Ok(crate::operation::OperationPlan {
                steps: vec![submit_pyth_update(
                    &ctx,
                    request.signer_account_id,
                    request.body.oracle_id,
                    request.body.vaa.0,
                )?],
            })
        })
    }
}

impl PlanWrite<GatewayContext> for oracle::UpdateRedStone {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            let oracle_id = request.body.oracle_id;
            let feed_id = request.body.feed_id;
            let payload = ctx
                .redstone_bridge()
                .fetch_payload(vec![feed_id.clone()])
                .await?;
            Ok(crate::operation::OperationPlan {
                steps: vec![submit_redstone_update(
                    &ctx,
                    request.signer_account_id,
                    oracle_id,
                    vec![feed_id],
                    payload,
                )?],
            })
        })
    }
}

impl PlanWrite<GatewayContext> for oracle::UpdatePrices {
    fn plan(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<crate::operation::OperationPlan>> {
        Box::pin(async move {
            let requests =
                resolve_update_requests(&ctx, request.body.oracle_id, request.body.price_ids)
                    .await?;

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
                let vaa = OraclePayloadSource::fetch_payload(ctx.pyth_http(), &price_ids).await?;
                let tx_result =
                    submit_pyth_update(&ctx, request.signer_account_id.clone(), oracle_id, vaa)?;
                steps.push(tx_result);
            }

            for (oracle_id, feed_ids) in redstone_updates {
                let feed_ids = feed_ids.into_iter().collect::<Vec<_>>();
                let payload = ctx
                    .redstone_bridge();
                let payload = OraclePayloadSource::fetch_payload(payload, &feed_ids)
                    .await?;
                let tx_result = submit_redstone_update(
                    &ctx,
                    request.signer_account_id.clone(),
                    oracle_id,
                    feed_ids,
                    payload,
                )?;
                steps.push(tx_result);
            }

            Ok(crate::operation::OperationPlan { steps })
        })
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

async fn query_oracle_kind(
    ctx: &GatewayContext,
    oracle_id: AccountId,
) -> GatewayResult<OracleContractKind> {
    match query_contract_kind(ctx, oracle_id.clone()).await? {
        templar_gateway_types::contract::ContractKind::PythOracle
        | templar_gateway_types::contract::ContractKind::RedstoneOracle => {
            Ok(OracleContractKind::Direct)
        }
        templar_gateway_types::contract::ContractKind::ProxyOracle => Ok(OracleContractKind::Proxy),
        templar_gateway_types::contract::ContractKind::LstOracle => {
            let pyth_id = ctx.lst_oracle(oracle_id).cached_oracle_id().await?;
            Ok(OracleContractKind::Lst { pyth_id })
        }
        other => Err(GatewayError::NearQuery(format!(
            "contract kind {other:?} is not an oracle contract"
        ))),
    }
}

async fn resolve_dependencies(
    ctx: &GatewayContext,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    kind: &OracleContractKind,
) -> GatewayResult<Vec<OracleRequest>> {
    match kind.clone() {
        OracleContractKind::Direct => Ok(vec![OracleRequest::pyth(oracle_id, price_id)]),
        OracleContractKind::Lst { pyth_id } => {
            let transformer = ctx
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

async fn resolve_price(
    ctx: &GatewayContext,
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

async fn get_price_onchain(
    ctx: &GatewayContext,
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

async fn resolve_proxy_entry_price(
    ctx: &GatewayContext,
    inputs: &ResolutionInputs,
    entry: &templar_common::oracle::proxy::Entry,
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

async fn resolve_proxy_entry_price_onchain(
    ctx: &GatewayContext,
    entry: &templar_common::oracle::proxy::Entry,
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

async fn fetch_transformer_input(
    ctx: &GatewayContext,
    call: price_transformer::Call,
) -> GatewayResult<Decimal> {
    ctx.contract(call.account_id)
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

async fn fetch_oracle_request_onchain(
    ctx: &GatewayContext,
    request: OracleRequest,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let fetched_price = match request {
        OracleRequest::Pyth(request) => ctx
            .pyth_oracle(request.oracle_id)
            .list_ema_prices_no_older_than(ListEmaPricesNoOlderThanArgs {
                price_ids: vec![request.price_id],
                age: max_age.as_secs(),
            })
            .await?
            .remove(&request.price_id)
            .flatten(),
        OracleRequest::RedStone(request) => ctx
            .redstone_oracle(request.oracle_id)
            .read_price_data(ReadPriceDataArgs {
                feed_ids: vec![request.price_id.clone()],
            })
            .await?
            .remove(&request.price_id)
            .and_then(|feed| feed.to_pyth_price()),
    };
    Ok(fetched_price.and_then(|p| validate_price_age(p, max_age)))
}

fn validate_price_age(price: pyth::Price, max_age: Nanoseconds) -> Option<pyth::Price> {
    let publish_time = Nanoseconds::try_from_pyth(price.publish_time)?;
    let now = system_time();
    if now >= publish_time && now.saturating_sub(publish_time) > max_age {
        return None;
    }
    Some(price)
}

async fn resolve_update_requests(
    ctx: &GatewayContext,
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

fn submit_pyth_update(
    ctx: &GatewayContext,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    oracle_id: AccountId,
    vaa: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    ctx.pyth_oracle(oracle_id).update_price_feeds(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .deposit(PYTH_UPDATE_DEPOSIT),
        UpdatePriceFeedsArgs {
            data: hex::encode(vaa),
        },
    )
}

fn submit_redstone_update(
    ctx: &GatewayContext,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    oracle_id: AccountId,
    feed_ids: Vec<redstone::FeedId>,
    payload: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    ctx.redstone_oracle(oracle_id).write_prices(
        ContractWriteOptions::new(signer_account_id).tgas(300),
        WritePricesArgs {
            feed_ids,
            payload: Base64VecU8(payload),
        },
    )
}

fn system_time() -> Nanoseconds {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Nanoseconds::from_ns(u64::try_from(now).unwrap_or(u64::MAX))
}
