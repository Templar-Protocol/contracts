use futures::future::BoxFuture;
use templar_gateway_types::lst_oracle;

use crate::DispatchRead;
use crate::{
    client::lst_oracle::{GetTransformerArgs, ListTransformersArgs},
    GatewayResult, HasNearClient,
};

impl<C: HasNearClient> DispatchRead<C> for lst_oracle::GetOracleId {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let pyth_oracle_id = ctx
                .near_client()
                .lst_oracle(request.params.oracle_id)
                .cached_oracle_id()
                .await?;
            Ok(lst_oracle::GetOracleIdResult { pyth_oracle_id })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for lst_oracle::ListTransformers {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let price_ids = ctx
                .near_client()
                .lst_oracle(request.params.oracle_id)
                .list_transformers(ListTransformersArgs {
                    offset: request.params.pagination.offset,
                    count: request.params.pagination.limit,
                })
                .await?;
            Ok(lst_oracle::ListTransformersResult { price_ids })
        })
    }
}

impl<C: HasNearClient> DispatchRead<C> for lst_oracle::GetTransformer {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let transformer = ctx
                .near_client()
                .lst_oracle(request.params.oracle_id)
                .cached_get_transformer(GetTransformerArgs {
                    price_identifier: request.params.price_identifier,
                })
                .await?;
            Ok(lst_oracle::GetTransformerResult { transformer })
        })
    }
}
