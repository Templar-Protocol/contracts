use blockchain_gateway_core::market;

use crate::{GatewayResult, GatewayService};

pub async fn get_configuration(
    service: &GatewayService,
    params: market::GetConfigurationParams,
) -> GatewayResult<market::GetConfigurationResult> {
    service.near().market(params.market_id).get_configuration(()).await
}

pub async fn list_borrow_positions(
    service: &GatewayService,
    params: market::ListBorrowPositionsParams,
) -> GatewayResult<market::ListBorrowPositionsResult> {
    let positions = service
        .near()
        .market(params.market_id)
        .list_borrow_positions(params.args)
        .await?;

    Ok(market::ListBorrowPositionsResult { positions })
}
