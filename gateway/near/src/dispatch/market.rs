use blockchain_gateway_core::market;
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, RpcMessage},
    GatewayResult, NearClient,
};

impl DispatchRead for market::GetConfiguration {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(params.0.params.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl DispatchRead for market::ListBorrowPositions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .market(params.market_id)
                .list_borrow_positions(params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}
