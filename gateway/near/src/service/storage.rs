use blockchain_gateway_core::storage;
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn get_balance_bounds(
    service: &GatewayService,
    params: storage::GetBalanceBoundsParams,
) -> BoxFuture<'_, crate::GatewayResult<storage::GetBalanceBoundsResult>> {
    Box::pin(async move {
        let bounds = service
            .near()
            .storage(params.contract_id)
            .storage_balance_bounds(params.args)
            .await?;

        Ok(storage::GetBalanceBoundsResult {
            bounds: blockchain_gateway_core::common::StorageBalanceBounds {
                min: bounds.min,
                max: bounds.max,
            },
        })
    })
}

pub fn get_balance_of(
    service: &GatewayService,
    params: storage::GetBalanceOfParams,
) -> BoxFuture<'_, crate::GatewayResult<storage::GetBalanceOfResult>> {
    Box::pin(async move {
        let balance = service
            .near()
            .storage(params.contract_id)
            .storage_balance_of(params.args)
            .await?
            .map(|balance| blockchain_gateway_core::common::StorageBalance {
                total: balance.total,
                available: balance.available,
            });

        Ok(storage::GetBalanceOfResult { balance })
    })
}
