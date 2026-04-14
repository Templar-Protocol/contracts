use blockchain_gateway_core::chain;
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn view_account(
    service: &GatewayService,
    params: chain::ViewAccountParams,
) -> BoxFuture<'_, crate::GatewayResult<chain::ViewAccountResult>> {
    Box::pin(async move { service.read().request(params).await })
}

pub fn view_function(
    service: &GatewayService,
    params: chain::ViewFunctionParams,
) -> BoxFuture<'_, crate::GatewayResult<chain::ViewFunctionResult>> {
    Box::pin(async move { service.read().request(params).await })
}

pub fn get_transaction(
    service: &GatewayService,
    params: chain::GetTransactionParams,
) -> BoxFuture<'_, crate::GatewayResult<chain::GetTransactionResult>> {
    Box::pin(async move { service.read().request(params).await })
}
