//! Shared oracle-kind resolution used by both the methods dispatch (price reads and
//! `oracle.getPriceResolutionDependencies`) and the oracle-updates dispatch (write
//! planning). Keeping it in one place is what guarantees reads, dependency metadata, and
//! writes agree on how an oracle resolves.
//!
//! A Pyth Lazer adapter is not a standalone oracle here: it is reachable only as a
//! proxy `Lazer` source (addressed by its native `u32` feed id, carried directly in the
//! request — no feed-map lookup). Targeting a bare adapter as a top-level oracle is
//! rejected, and a proxy that references one as a classic `Pyth` source is rejected too.

use std::collections::BTreeSet;

use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_types::{ContractKind, OracleContractKind};
use templar_proxy_oracle_kernel::proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_common::request::{OracleRequest, PythRequest};

use crate::client::{lst_oracle::GetTransformerArgs, proxy_oracle::GetProxyArgs};
use crate::{query_contract_kind, GatewayError, GatewayResult, HasNearClient};

/// Resolve an oracle contract's [`ContractKind`] into the refined, resolution-facing
/// [`OracleContractKind`] (reading the underlying Pyth id for LST oracles).
pub async fn query_oracle_kind<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
) -> GatewayResult<OracleContractKind> {
    match query_contract_kind(ctx, oracle_id.clone()).await? {
        ContractKind::PythOracle | ContractKind::RedstoneOracle => Ok(OracleContractKind::Direct),
        ContractKind::PythLazerOracle => Err(GatewayError::NearQuery(format!(
            "pyth-lazer adapter {oracle_id} is not usable as a standalone oracle; wrap it in a \
             proxy oracle with a Lazer source"
        ))),
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

/// Fetch a proxy oracle's (cached) proxy definition for `price_id`.
pub async fn get_proxy<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_id: PriceIdentifier,
) -> GatewayResult<Option<proxy::Proxy<Source>>> {
    ctx.near_client()
        .proxy_oracle(oracle_id)
        .cached_get_proxy(GetProxyArgs { id: price_id })
        .await
}

/// The underlying source payload(s) a single `price_id` on `oracle_id` resolves to — the
/// dependencies that must be fetched and submitted to refresh it. A direct Pyth Lazer
/// adapter maps its consumer `PriceIdentifier` to a Lazer feed via the adapter's feed map.
pub async fn resolve_price_dependencies<C: HasNearClient>(
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
            // A Pyth Lazer adapter is Lazer-native and must be referenced by feed id. Reject a
            // proxy source that wires one as a classic `Pyth` source (by `PriceIdentifier`):
            // the gateway would otherwise plan a Pyth VAA write the adapter's ABI rejects.
            // Deduplicate the Pyth oracle ids first so a single adapter referenced by several
            // sources/price ids is only probed once.
            let pyth_oracle_ids = requests
                .iter()
                .filter_map(|request| match request {
                    OracleRequest::Pyth(PythRequest { oracle_id, .. }) => Some(oracle_id.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            for oracle_id in pyth_oracle_ids {
                if query_contract_kind(ctx, oracle_id.clone()).await?
                    == ContractKind::PythLazerOracle
                {
                    return Err(GatewayError::NearQuery(format!(
                        "proxy source references Pyth Lazer adapter {oracle_id} as a classic \
                         Pyth source; use OracleRequest::Lazer (by feed id) — the adapter \
                         is Lazer-native"
                    )));
                }
            }
            Ok(requests)
        }
    }
}
