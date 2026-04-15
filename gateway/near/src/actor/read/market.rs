use blockchain_gateway_core::market;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearReadClient};

use super::ReadRpcRequest;
use crate::actor::rpc::RpcMessage;

impl ReadRpcRequest for market::GetConfiguration {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(params.0.body.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl ReadRpcRequest for market::ListBorrowPositions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .market(params.market_id)
                .list_borrow_positions(params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}
