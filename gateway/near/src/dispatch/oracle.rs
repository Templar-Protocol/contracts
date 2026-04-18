use std::collections::{BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use blockchain_gateway_core::oracle::{
    self, GetPriceResolutionDependenciesResult, OracleContractKind, RedStoneOraclePrices,
    RedStonePriceEntry, ResolvePricesResult, ResolvedPrice,
};
use futures::future::BoxFuture;
use near_account_id::AccountId;
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

use crate::{
    actor::DispatchRead,
    client::{
        lst_oracle::GetTransformerArgs,
        proxy_oracle::{GetProxyArgs, ListProxiesArgs},
        pyth_oracle::ListEmaPricesNoOlderThanArgs,
        redstone_oracle::ReadPriceDataArgs,
    },
    GatewayError, GatewayResult, NearClient,
};

async fn get_proxy(
    client: &NearClient,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<templar_common::oracle::proxy::Proxy>> {
    client
        .proxy_oracle(oracle_id)
        .get_proxy(GetProxyArgs { id })
        .await
}

impl DispatchRead for oracle::GetKind {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { query_oracle_kind(&client, request.params.oracle_id).await })
    }
}

impl DispatchRead for oracle::GetPriceResolutionDependencies {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let kind = query_oracle_kind(&client, params.oracle_id.clone()).await?;
            let requests =
                resolve_dependencies(&client, params.oracle_id, params.price_id, &kind).await?;
            Ok(GetPriceResolutionDependenciesResult { kind, requests })
        })
    }
}

impl DispatchRead for oracle::ResolvePrice {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let inputs = ResolutionInputs::new(params.pyth, params.redstone);
            let price = resolve_price(
                &client,
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

impl DispatchRead for oracle::ResolvePrices {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let inputs = ResolutionInputs::new(params.pyth, params.redstone);
            let max_age = Nanoseconds::from_secs(params.age);
            let mut prices = Vec::with_capacity(params.price_ids.len());
            for price_id in params.price_ids {
                let price = resolve_price(
                    &client,
                    &inputs,
                    params.oracle_id.clone(),
                    price_id,
                    max_age,
                )
                .await?;
                prices.push(ResolvedPrice { price_id, price });
            }
            Ok(ResolvePricesResult { prices })
        })
    }
}

impl DispatchRead for oracle::GetPrice {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let price = get_price_onchain(
                &client,
                params.oracle_id,
                params.price_id,
                Nanoseconds::from_secs(params.age),
            )
            .await?;
            Ok(oracle::GetPriceResult { price })
        })
    }
}

impl DispatchRead for oracle::GetPrices {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = request.params;
            let max_age = Nanoseconds::from_secs(params.age);
            let mut prices = Vec::with_capacity(params.price_ids.len());
            for price_id in params.price_ids {
                let price =
                    get_price_onchain(&client, params.oracle_id.clone(), price_id, max_age).await?;
                prices.push(ResolvedPrice { price_id, price });
            }
            Ok(ResolvePricesResult { prices })
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
    client: &NearClient,
    oracle_id: AccountId,
) -> GatewayResult<OracleContractKind> {
    if client
        .proxy_oracle(oracle_id.clone())
        .list_proxies(ListProxiesArgs {
            offset: None,
            count: Some(1),
        })
        .await
        .is_ok()
    {
        return Ok(OracleContractKind::Proxy);
    }

    match client
        .lst_oracle(oracle_id.clone())
        .list_transformers(crate::client::lst_oracle::ListTransformersArgs {
            offset: None,
            count: Some(1),
        })
        .await
    {
        Ok(_) => {
            let pyth_id = client.lst_oracle(oracle_id).oracle_id(()).await?;
            Ok(OracleContractKind::Lst { pyth_id })
        }
        Err(error) if is_method_not_found(&error) => Ok(OracleContractKind::Direct),
        Err(error) => Err(error),
    }
}

async fn resolve_dependencies(
    client: &NearClient,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    kind: &OracleContractKind,
) -> GatewayResult<Vec<OracleRequest>> {
    match kind.clone() {
        OracleContractKind::Direct => Ok(vec![OracleRequest::pyth(oracle_id, price_id)]),
        OracleContractKind::Lst { pyth_id } => {
            let transformer = client
                .lst_oracle(oracle_id)
                .get_transformer(GetTransformerArgs {
                    price_identifier: price_id,
                })
                .await?;
            Ok(vec![transformer.map_or_else(
                || OracleRequest::pyth(pyth_id.clone(), price_id),
                |transformer| OracleRequest::pyth(pyth_id.clone(), transformer.price_id),
            )])
        }
        OracleContractKind::Proxy => {
            let proxy = get_proxy(client, oracle_id, price_id)
                .await?
                .ok_or_else(|| {
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
    client: &NearClient,
    inputs: &ResolutionInputs,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let kind = query_oracle_kind(client, oracle_id.clone()).await?;
    match kind {
        OracleContractKind::Direct => Ok(fetch_oracle_request(
            inputs,
            OracleRequest::pyth(oracle_id, price_id),
            max_age,
        )),
        OracleContractKind::Lst { pyth_id } => {
            let transformer = client
                .lst_oracle(oracle_id)
                .get_transformer(GetTransformerArgs {
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
                    let input = fetch_transformer_input(client, transformer.call).await?;
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
            let proxy = get_proxy(client, oracle_id, price_id)
                .await?
                .ok_or_else(|| {
                    GatewayError::NearQuery("price identifier not found on proxy oracle".to_owned())
                })?;
            let mut prices = vec![];
            for entry in &proxy.entries {
                if let Some(price) =
                    resolve_proxy_entry_price(client, inputs, entry, max_age).await?
                {
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
    client: &NearClient,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let kind = query_oracle_kind(client, oracle_id.clone()).await?;
    match kind {
        OracleContractKind::Direct => {
            fetch_oracle_request_onchain(client, OracleRequest::pyth(oracle_id, price_id), max_age)
                .await
        }
        OracleContractKind::Lst { pyth_id } => {
            let transformer = client
                .lst_oracle(oracle_id.clone())
                .get_transformer(GetTransformerArgs {
                    price_identifier: price_id,
                })
                .await?;
            match transformer {
                Some(transformer) => {
                    let Some(price) = fetch_oracle_request_onchain(
                        client,
                        OracleRequest::pyth(pyth_id, transformer.price_id),
                        max_age,
                    )
                    .await?
                    else {
                        return Ok(None);
                    };
                    let input = fetch_transformer_input(client, transformer.call).await?;
                    Ok(transformer.action.apply(price, input))
                }
                None => {
                    fetch_oracle_request_onchain(
                        client,
                        OracleRequest::pyth(pyth_id, price_id),
                        max_age,
                    )
                    .await
                }
            }
        }
        OracleContractKind::Proxy => {
            let proxy = get_proxy(client, oracle_id, price_id)
                .await?
                .ok_or_else(|| {
                    GatewayError::NearQuery("price identifier not found on proxy oracle".to_owned())
                })?;
            let mut prices = vec![];
            for entry in &proxy.entries {
                if let Some(price) =
                    resolve_proxy_entry_price_onchain(client, entry, max_age).await?
                {
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
    client: &NearClient,
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
            let input = fetch_transformer_input(client, transformer.call.clone()).await?;
            Ok(transformer.action.apply(price, input))
        }
    }
}

async fn resolve_proxy_entry_price_onchain(
    client: &NearClient,
    entry: &templar_common::oracle::proxy::Entry,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    match &entry.source {
        Source::Request(request) => {
            fetch_oracle_request_onchain(client, request.clone(), max_age).await
        }
        Source::Transformer(transformer) => {
            let Some(price) =
                fetch_oracle_request_onchain(client, transformer.request.clone(), max_age).await?
            else {
                return Ok(None);
            };
            let input = fetch_transformer_input(client, transformer.call.clone()).await?;
            Ok(transformer.action.apply(price, input))
        }
    }
}

async fn fetch_transformer_input(
    client: &NearClient,
    call: price_transformer::Call,
) -> GatewayResult<Decimal> {
    client
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

async fn fetch_oracle_request_onchain(
    client: &NearClient,
    request: OracleRequest,
    max_age: Nanoseconds,
) -> GatewayResult<Option<pyth::Price>> {
    let fetched_price = match request {
        OracleRequest::Pyth(request) => client
            .pyth_oracle(request.oracle_id)
            .list_ema_prices_no_older_than(ListEmaPricesNoOlderThanArgs {
                price_ids: vec![request.price_id],
                age: max_age.as_secs(),
            })
            .await?
            .remove(&request.price_id)
            .flatten(),
        OracleRequest::RedStone(request) => client
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

fn is_method_not_found(error: &GatewayError) -> bool {
    matches!(error, GatewayError::NearQuery(message) if message.contains("MethodNotFound"))
}

fn system_time() -> Nanoseconds {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Nanoseconds::from_ns(u64::try_from(now).unwrap_or(u64::MAX))
}
