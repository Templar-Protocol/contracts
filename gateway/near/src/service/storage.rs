use blockchain_gateway_core::storage;

use crate::{GatewayResult, GatewayService};

pub async fn get_balance_bounds(
    service: &GatewayService,
    params: storage::GetBalanceBoundsParams,
) -> GatewayResult<storage::GetBalanceBoundsResult> {
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
}

pub async fn get_balance_of(
    service: &GatewayService,
    params: storage::GetBalanceOfParams,
) -> GatewayResult<storage::GetBalanceOfResult> {
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
}
