use blockchain_gateway_core::market;
use futures::future::BoxFuture;

use crate::{actor::DispatchRead, GatewayResult, NearClient};

impl DispatchRead for market::GetConfiguration {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .get_configuration(())
                .await
        })
    }
}

impl DispatchRead for market::ListBorrowPositions {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .market(request.params.market_id)
                .list_borrow_positions(request.params.args)
                .await
                .map(|positions| market::ListBorrowPositionsResult { positions })
        })
    }
}
