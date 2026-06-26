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
