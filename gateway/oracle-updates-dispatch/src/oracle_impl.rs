use std::collections::BTreeSet;

use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_core::{
    client::{lst_oracle::GetTransformerArgs, proxy_oracle::GetProxyArgs},
    plan_pyth_update, plan_redstone_write_prices, query_contract_kind, GatewayError, GatewayResult,
    HasNearClient, OperationPlan, OraclePayloadSource, PlanWrite,
};
use templar_gateway_methods_spec::oracle::OracleContractKind;
use templar_gateway_oracle_updates_spec::oracle::{UpdatePrices, UpdatePyth, UpdateRedStone};
use templar_gateway_types::ContractKind;
use templar_proxy_oracle_kernel::proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_common::request::OracleRequest;

use crate::{Dispatch, ProvidesPythSource, ProvidesRedStoneSource};

#[async_trait]
impl<C> PlanWrite<UpdatePyth, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdatePyth>,
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
impl<C> PlanWrite<UpdateRedStone, C> for Dispatch
where
    C: HasNearClient + ProvidesRedStoneSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdateRedStone>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        let feed_id = body.feed_id;
        tracing::debug!(
            oracle_id = %body.oracle_id,
            feed_id = %feed_id,
            "fetching RedStone payload for gateway oracle update"
        );
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
impl<C> PlanWrite<UpdatePrices, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdatePrices>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let requests =
            resolve_update_requests(&ctx, request.body.oracle_id, request.body.price_ids).await?;

        let mut steps = Vec::new();
        let mut pyth_updates =
            std::collections::BTreeMap::<AccountId, BTreeSet<PriceIdentifier>>::new();
        let mut redstone_updates =
            std::collections::BTreeMap::<AccountId, BTreeSet<redstone::FeedId>>::new();

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

        tracing::debug!(
            pyth_oracle_count = pyth_updates.len(),
            redstone_oracle_count = redstone_updates.len(),
            "resolved oracle update dependencies"
        );

        for (oracle_id, price_ids) in pyth_updates {
            let price_ids = price_ids.into_iter().collect::<Vec<_>>();
            tracing::debug!(
                %oracle_id,
                price_count = price_ids.len(),
                "fetching Pyth payload for gateway oracle update"
            );
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
            tracing::debug!(
                %oracle_id,
                feed_count = feed_ids.len(),
                "fetching RedStone payload for gateway oracle update"
            );
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
