use async_trait::async_trait;
use templar_gateway_core::{
    client::lst_oracle::{GetTransformerArgs, ListTransformersArgs},
    DispatchRead, GatewayResult, HasNearClient,
};
use templar_gateway_methods_spec::lst_oracle;
use templar_gateway_types::MethodSpec;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::GetOracleId, C> for Dispatch {
    async fn dispatch(
        request: <lst_oracle::GetOracleId as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<lst_oracle::GetOracleIdResult> {
        let pyth_oracle_id = ctx
            .near_client()
            .lst_oracle(request.params.oracle_id)
            .cached_oracle_id()
            .await?;
        Ok(lst_oracle::GetOracleIdResult { pyth_oracle_id })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::ListTransformers, C> for Dispatch {
    async fn dispatch(
        request: <lst_oracle::ListTransformers as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<lst_oracle::ListTransformersResult> {
        let price_ids = ctx
            .near_client()
            .lst_oracle(request.params.oracle_id)
            .list_transformers(ListTransformersArgs {
                offset: request.params.pagination.offset,
                count: request.params.pagination.limit,
            })
            .await?;
        Ok(lst_oracle::ListTransformersResult { price_ids })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<lst_oracle::GetTransformer, C> for Dispatch {
    async fn dispatch(
        request: <lst_oracle::GetTransformer as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<lst_oracle::GetTransformerResult> {
        let transformer = ctx
            .near_client()
            .lst_oracle(request.params.oracle_id)
            .cached_get_transformer(GetTransformerArgs {
                price_identifier: request.params.price_identifier,
            })
            .await?;
        Ok(lst_oracle::GetTransformerResult { transformer })
    }
}
