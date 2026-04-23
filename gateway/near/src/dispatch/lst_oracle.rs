use futures::future::BoxFuture;
use templar_gateway_types::lst_oracle;

use crate::{
    actor::DispatchRead,
    client::lst_oracle::{GetTransformerArgs, ListTransformersArgs},
    GatewayContext, GatewayResult,
};

impl DispatchRead for lst_oracle::GetOracleId {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let pyth_oracle_id = ctx
                .lst_oracle(request.params.oracle_id)
                .cached_oracle_id()
                .await?;
            Ok(lst_oracle::GetOracleIdResult { pyth_oracle_id })
        })
    }
}

impl DispatchRead for lst_oracle::ListTransformers {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let price_ids = ctx
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

impl DispatchRead for lst_oracle::GetTransformer {
    fn dispatch(
        request: Self::Input,
        ctx: GatewayContext,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let transformer = ctx
                .lst_oracle(request.params.oracle_id)
                .cached_get_transformer(GetTransformerArgs {
                    price_identifier: request.params.price_identifier,
                })
                .await?;
            Ok(lst_oracle::GetTransformerResult { transformer })
        })
    }
}
