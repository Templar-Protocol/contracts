use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_core::{
    client::lst_oracle::{CreateTransformerArgs, GetTransformerArgs, ListTransformersArgs},
    client::ContractWriteOptions,
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite,
};
use templar_gateway_methods_spec::lst_oracle;
use templar_proxy_oracle_near_common::price_transformer::PriceTransformer;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::GetOracleId, C> for Dispatch {
    async fn dispatch(request: lst_oracle::GetOracleId, ctx: C) -> GatewayResult<AccountId> {
        ctx.near_client()
            .lst_oracle(request.oracle_id)
            .cached_oracle_id()
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::ListTransformers, C> for Dispatch {
    async fn dispatch(
        request: lst_oracle::ListTransformers,
        ctx: C,
    ) -> GatewayResult<Vec<PriceIdentifier>> {
        ctx.near_client()
            .lst_oracle(request.oracle_id)
            .list_transformers(ListTransformersArgs {
                offset: request.pagination.offset,
                count: request.pagination.limit,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::GetTransformer, C> for Dispatch {
    async fn dispatch(
        request: lst_oracle::GetTransformer,
        ctx: C,
    ) -> GatewayResult<Option<PriceTransformer>> {
        // Uncached, for the same reason as `proxyOracle.getProxy`: no write path
        // invalidates the transformer cache, and `load_cached` caches a `None`
        // too — so a point read taken before `create_transformer` would keep
        // reporting the transformer absent for `CONFIG_CACHE_TTL` after it
        // exists. The oracle-resolution paths keep the cache.
        ctx.near_client()
            .lst_oracle(request.oracle_id)
            .get_transformer(GetTransformerArgs {
                price_identifier: request.price_identifier,
            })
            .await
    }
}

#[async_trait]
impl<C: HasNearClient> PlanWrite<lst_oracle::CreateTransformer, C> for Dispatch {
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<lst_oracle::CreateTransformer>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        ctx.near_client()
            .lst_oracle(body.oracle_id)
            .create_transformer(
                ContractWriteOptions::new(request.signer_account_id)
                    .tgas(100)
                    .one_yocto(),
                CreateTransformerArgs {
                    price_identifier: body.price_identifier,
                    entry: body.entry,
                },
            )
            .map(OperationPlan::from)
    }
}
