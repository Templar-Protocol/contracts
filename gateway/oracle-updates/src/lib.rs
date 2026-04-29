use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use async_trait::async_trait;
use near_account_id::AccountId;
use near_sdk::{json_types::Base64VecU8, NearToken};
use templar_common::oracle::{proxy, pyth::PriceIdentifier, redstone, OracleRequest};
use templar_gateway_core::{
    client::{pyth_oracle::UpdatePriceFeedsArgs, redstone_oracle::WritePricesArgs},
    query_contract_kind, ContractWriteOptions, GatewayContextBuilder, GatewayError, GatewayResult,
    HasNearClient, OperationPlan, PlanWrite, PlannedTransaction, ProvidesPythSource,
    ProvidesRedStoneSource,
};
use templar_gateway_types::oracle::OracleContractKind;
use templar_gateway_types::{contract::ContractKind, oracle, MethodSpec, OraclePayloadSource};
use url::Url;

pub use templar_gateway_oracle_pyth::PythHttpClient;
pub use templar_gateway_oracle_redstone::RedStoneBridgeClient;

pub mod prelude {
    pub use crate::GatewayContextBuilderOracleExt;
}

pub struct Dispatch;

const PYTH_UPDATE_DEPOSIT: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000_000);

#[derive(Debug, Clone)]
pub struct WithPythSource<C> {
    inner: C,
    pyth_source: PythHttpClient,
}

#[derive(Debug, Clone)]
pub struct WithRedStoneSource<C> {
    inner: C,
    redstone_source: RedStoneBridgeClient,
}

pub trait GatewayContextBuilderOracleExt<C>: Sized {
    fn with_pyth_source(self, pyth_hermes_url: Url) -> GatewayContextBuilder<WithPythSource<C>>;

    fn with_redstone_source(
        self,
        redstone_node_path: impl AsRef<Path>,
    ) -> Result<GatewayContextBuilder<WithRedStoneSource<C>>, GatewayError>;
}

impl<C> GatewayContextBuilderOracleExt<C> for GatewayContextBuilder<C> {
    fn with_pyth_source(self, pyth_hermes_url: Url) -> GatewayContextBuilder<WithPythSource<C>> {
        self.map(|inner| WithPythSource {
            inner,
            pyth_source: PythHttpClient::new(pyth_hermes_url),
        })
    }

    fn with_redstone_source(
        self,
        redstone_node_path: impl AsRef<Path>,
    ) -> Result<GatewayContextBuilder<WithRedStoneSource<C>>, GatewayError> {
        let redstone_source = RedStoneBridgeClient::new(redstone_node_path.as_ref())
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        Ok(self.map(|inner| WithRedStoneSource {
            inner,
            redstone_source,
        }))
    }
}

impl<C: HasNearClient> HasNearClient for WithPythSource<C> {
    fn near_client(&self) -> &templar_gateway_core::NearClient {
        self.inner.near_client()
    }
}

impl<C> ProvidesPythSource for WithPythSource<C> {
    type PythSource = PythHttpClient;

    fn pyth_source(&self) -> &Self::PythSource {
        &self.pyth_source
    }
}

impl<C: HasNearClient> HasNearClient for WithRedStoneSource<C> {
    fn near_client(&self) -> &templar_gateway_core::NearClient {
        self.inner.near_client()
    }
}

impl<C: ProvidesPythSource> ProvidesPythSource for WithRedStoneSource<C> {
    type PythSource = C::PythSource;

    fn pyth_source(&self) -> &Self::PythSource {
        self.inner.pyth_source()
    }
}

impl<C> ProvidesRedStoneSource for WithRedStoneSource<C> {
    type RedStoneSource = RedStoneBridgeClient;

    fn redstone_source(&self) -> &Self::RedStoneSource {
        &self.redstone_source
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
        submit_pyth_update(
            &ctx,
            request.signer_account_id,
            request.body.oracle_id,
            request.body.vaa.0,
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
        let oracle_id = request.body.oracle_id;
        let feed_id = request.body.feed_id;
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &[feed_id.clone()])
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        submit_redstone_update(
            &ctx,
            request.signer_account_id,
            oracle_id,
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
            steps.push(submit_pyth_update(
                &ctx,
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
            steps.push(submit_redstone_update(
                &ctx,
                request.signer_account_id.clone(),
                oracle_id,
                feed_ids,
                payload,
            )?);
        }

        Ok(OperationPlan { steps })
    }
}

fn submit_pyth_update<C: HasNearClient>(
    ctx: &C,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    oracle_id: AccountId,
    vaa: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    ctx.near_client().pyth_oracle(oracle_id).update_price_feeds(
        ContractWriteOptions::new(signer_account_id)
            .tgas(300)
            .deposit(PYTH_UPDATE_DEPOSIT),
        UpdatePriceFeedsArgs {
            data: hex::encode(vaa),
        },
    )
}

fn submit_redstone_update<C: HasNearClient>(
    ctx: &C,
    signer_account_id: templar_gateway_types::ManagedAccountId,
    oracle_id: AccountId,
    feed_ids: Vec<redstone::FeedId>,
    payload: Vec<u8>,
) -> GatewayResult<PlannedTransaction> {
    ctx.near_client().redstone_oracle(oracle_id).write_prices(
        ContractWriteOptions::new(signer_account_id).tgas(300),
        WritePricesArgs {
            feed_ids,
            payload: Base64VecU8(payload),
        },
    )
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
                .cached_get_transformer(
                    templar_gateway_core::client::lst_oracle::GetTransformerArgs {
                        price_identifier: price_id,
                    },
                )
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
                    proxy::Source::Request(request) => request,
                    proxy::Source::Transformer(transformer) => transformer.request,
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

async fn get_proxy<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    id: PriceIdentifier,
) -> GatewayResult<Option<proxy::Proxy>> {
    ctx.near_client()
        .proxy_oracle(oracle_id)
        .cached_get_proxy(templar_gateway_core::client::proxy_oracle::GetProxyArgs { id })
        .await
}
