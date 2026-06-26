use async_trait::async_trait;
use templar_gateway_core::{DispatchRead, GatewayResult, HasNearClient};
use templar_gateway_methods_spec::chain;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<chain::GetGasPrice, C> for Dispatch {
    async fn dispatch(
        _request: chain::GetGasPrice,
        ctx: C,
    ) -> GatewayResult<chain::GetGasPriceResult> {
        ctx.near_client()
            .chain()
            .gas_price()
            .await
            .map(|gas_price| chain::GetGasPriceResult { gas_price })
    }
}

#[async_trait]
impl<C: HasNearClient> DispatchRead<chain::GetBlock, C> for Dispatch {
    async fn dispatch(request: chain::GetBlock, ctx: C) -> GatewayResult<chain::GetBlockResult> {
        ctx.near_client()
            .chain()
            .block(request.block_hash)
            .await
            .map(|(height, timestamp_ns)| chain::GetBlockResult {
                height,
                timestamp_ns,
            })
    }
}
