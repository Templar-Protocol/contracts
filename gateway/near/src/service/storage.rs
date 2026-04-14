use blockchain_gateway_core::{rpc::common::WriteRequest, storage};
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn get_balance_bounds(
    service: &GatewayService,
    params: storage::GetBalanceBoundsParams,
) -> BoxFuture<'_, crate::GatewayResult<storage::GetBalanceBoundsResult>> {
    Box::pin(async move { service.read().request(params).await })
}

pub fn get_balance_of(
    service: &GatewayService,
    params: storage::GetBalanceOfParams,
) -> BoxFuture<'_, crate::GatewayResult<storage::GetBalanceOfResult>> {
    Box::pin(async move { service.read().request(params).await })
}

pub fn deposit(
    service: &GatewayService,
    request: WriteRequest<storage::DepositBody>,
) -> BoxFuture<'_, crate::GatewayResult<storage::DepositResult>> {
    Box::pin(async move { service.write().request(request).await })
}
