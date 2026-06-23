use async_trait::async_trait;
use templar_gateway_core::{
    client::lst_oracle::{GetTransformerArgs, ListTransformersArgs},
    DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::lst_oracle;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::GetOracleId, C> for Dispatch {
    async fn dispatch(
        request: lst_oracle::GetOracleId,
        ctx: C,
    ) -> GatewayResult<lst_oracle::GetOracleIdResult> {
        let pyth_oracle_id = ctx
            .near_client()
            .lst_oracle(request.oracle_id)
            .cached_oracle_id()
            .await?;
        Ok(lst_oracle::GetOracleIdResult { pyth_oracle_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::ListTransformers, C> for Dispatch {
    async fn dispatch(
        request: lst_oracle::ListTransformers,
        ctx: C,
    ) -> GatewayResult<lst_oracle::ListTransformersResult> {
        let price_ids = ctx
            .near_client()
            .lst_oracle(request.oracle_id)
            .list_transformers(ListTransformersArgs {
                offset: request.pagination.offset,
                count: request.pagination.limit,
            })
            .await?;
        Ok(lst_oracle::ListTransformersResult { price_ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::GetTransformer, C> for Dispatch {
    async fn dispatch(
        request: lst_oracle::GetTransformer,
        ctx: C,
    ) -> GatewayResult<lst_oracle::GetTransformerResult> {
        let transformer = ctx
            .near_client()
            .lst_oracle(request.oracle_id)
            .cached_get_transformer(GetTransformerArgs {
                price_identifier: request.price_identifier,
            })
            .await?;
        Ok(lst_oracle::GetTransformerResult { transformer })
    }
}
