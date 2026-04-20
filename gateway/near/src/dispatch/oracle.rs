use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use blockchain_gateway_core::oracle::{
    self, GetPriceResolutionDependenciesResult, OracleContractKind, RedStoneOraclePrices,
    RedStonePriceEntry, ResolvePricesResult, ResolvedPrice,
};
use blockchain_gateway_core::{
    common::WriteOperationResult, OperationId, OperationRecord, OperationStatus, StepStatus,
    TransactionStepRecord,
};
use futures::future::BoxFuture;
use near_account_id::AccountId;
use near_api::types::transaction::result::TransactionResult;
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

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite},
    client::{
        lst_oracle::GetTransformerArgs,
        proxy_oracle::{GetProxyArgs, ListProxiesArgs},
        pyth_oracle::{ListEmaPricesNoOlderThanArgs, UpdatePriceFeedsArgs},
        redstone_oracle::{ReadPriceDataArgs, WritePricesArgs},
        ContractWriteOptions,
    },
    GatewayContext, GatewayError, GatewayResult,
};

const PYTH_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

async fn get_proxy(
    ctx: &GatewayContext,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<templar_common::oracle::proxy::Proxy>> {
    ctx.proxy_oracle(oracle_id)
        .get_proxy(GetProxyArgs { id })
        .await
}

impl DispatchRead for oracle::GetKind {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { query_oracle_kind(&ctx, request.params.oracle_id).await })
    }
}

impl DispatchRead for oracle::GetPriceResolutionDependencies {
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

impl DispatchRead for oracle::ResolvePrice {
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

impl DispatchRead for oracle::ResolvePrices {
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

impl DispatchRead for oracle::GetPrice {
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

impl DispatchRead for oracle::GetPrices {
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

impl DispatchWrite for oracle::UpdatePyth {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = submit_pyth_update(
                &ctx,
                request.signer_account_id,
                signer,
                request.wait_until,
                request.body.oracle_id,
                request.body.vaa.0,
            )
            .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for oracle::UpdateRedStone {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let oracle_id = request.body.oracle_id;
            let feed_id = request.body.feed_id;
            let payload = ctx
                .redstone_bridge()
                .fetch_payload(vec![feed_id.clone()])
                .await?;
            let tx_result = submit_redstone_update(
                &ctx,
                request.signer_account_id,
                signer,
                request.wait_until,
                oracle_id,
                vec![feed_id],
                payload,
            )
            .await?;
            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}

impl DispatchWrite for oracle::UpdatePrices {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let requests =
                resolve_update_requests(&ctx, request.body.oracle_id, request.body.price_ids)
                    .await?;

            let mut results = Vec::new();
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
                let vaa = ctx.pyth_http().fetch_latest_vaa(&price_ids).await?;
                let tx_result = submit_pyth_update(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    oracle_id,
                    vaa,
                )
                .await?;
                results.push(tx_result);
            }

            for (oracle_id, feed_ids) in redstone_updates {
                let feed_ids = feed_ids.into_iter().collect::<Vec<_>>();
                let payload = ctx
                    .redstone_bridge()
                    .fetch_payload(feed_ids.clone())
                    .await?;
                let tx_result = submit_redstone_update(
                    &ctx,
                    request.signer_account_id.clone(),
                    signer.clone(),
                    request.wait_until,
                    oracle_id,
                    feed_ids,
                    payload,
                )
                .await?;
                results.push(tx_result);
            }

            Ok(operation_outcome_from_transaction_results(
                signer_account_id,
                results,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
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
    if ctx
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

    match ctx
        .lst_oracle(oracle_id.clone())
        .list_transformers(crate::client::lst_oracle::ListTransformersArgs {
            offset: None,
            count: Some(1),
        })
        .await
    {
        Ok(_) => {
            let pyth_id = ctx.lst_oracle(oracle_id).oracle_id(()).await?;
            Ok(OracleContractKind::Lst { pyth_id })
        }
        Err(error) if is_method_not_found(&error) => Ok(OracleContractKind::Direct),
        Err(error) => Err(error),
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
                .get_transformer(GetTransformerArgs {
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

fn is_method_not_found(error: &GatewayError) -> bool {
    matches!(error, GatewayError::NearQuery(message) if message.contains("MethodNotFound"))
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

async fn submit_pyth_update(
    ctx: &GatewayContext,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    signer: Arc<near_api::Signer>,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    oracle_id: AccountId,
    vaa: Vec<u8>,
) -> GatewayResult<TransactionResult> {
    ctx.pyth_oracle(oracle_id)
        .update_price_feeds(
            ContractWriteOptions::new(signer_account_id, signer)
                .wait_until(wait_until)
                .tgas(300)
                .deposit(PYTH_UPDATE_DEPOSIT),
            UpdatePriceFeedsArgs {
                data: hex::encode(vaa),
            },
        )
        .await
}

async fn submit_redstone_update(
    ctx: &GatewayContext,
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    signer: Arc<near_api::Signer>,
    wait_until: blockchain_gateway_core::common::TxExecutionStatus,
    oracle_id: AccountId,
    feed_ids: Vec<redstone::FeedId>,
    payload: Vec<u8>,
) -> GatewayResult<TransactionResult> {
    ctx.redstone_oracle(oracle_id)
        .write_prices(
            ContractWriteOptions::new(signer_account_id, signer)
                .wait_until(wait_until)
                .tgas(300),
            WritePricesArgs {
                feed_ids,
                payload: Base64VecU8(payload),
            },
        )
        .await
}

fn operation_outcome_from_transaction_results(
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    tx_results: Vec<TransactionResult>,
) -> WriteOperationResult {
    let mut status = OperationStatus::Succeeded;
    let mut operation_id = None;
    let mut steps = Vec::with_capacity(tx_results.len());

    for (index, tx_result) in tx_results.into_iter().enumerate() {
        let step_status = if let Some(full) = tx_result.into_full() {
            let outcome = full.outcome();
            let tx_hash: blockchain_gateway_core::CryptoHash = outcome.transaction_hash.into();
            if operation_id.is_none() {
                operation_id = Some(tx_hash.0.to_string());
            }
            if full.is_success() {
                StepStatus::Succeeded { tx_hash }
            } else {
                status = OperationStatus::Failed;
                StepStatus::Failed {
                    tx_hash: Some(tx_hash),
                }
            }
        } else {
            if status != OperationStatus::Failed {
                status = OperationStatus::InProgress;
            }
            StepStatus::Submitted { tx_hash: None }
        };

        steps.push(TransactionStepRecord {
            index: index as u32,
            status: step_status,
        });
    }

    WriteOperationResult {
        operation: OperationRecord {
            id: OperationId(operation_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())),
            signer_account_id,
            status,
            steps,
        },
    }
}

fn system_time() -> Nanoseconds {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Nanoseconds::from_ns(u64::try_from(now).unwrap_or(u64::MAX))
}
