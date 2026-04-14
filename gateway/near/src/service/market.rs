use blockchain_gateway_core::market;
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn get_configuration(
    service: &GatewayService,
    params: market::GetConfigurationParams,
) -> BoxFuture<'_, crate::GatewayResult<market::GetConfigurationResult>> {
    Box::pin(async move {
        service
            .near()
            .market(params.market_id)
            .get_configuration(())
            .await
    })
}

pub fn list_borrow_positions(
    service: &GatewayService,
    params: market::ListBorrowPositionsParams,
) -> BoxFuture<'_, crate::GatewayResult<market::ListBorrowPositionsResult>> {
    Box::pin(async move {
        let positions = service
            .near()
            .market(params.market_id)
            .list_borrow_positions(params.args)
            .await?;

        Ok(market::ListBorrowPositionsResult { positions })
    })
}
