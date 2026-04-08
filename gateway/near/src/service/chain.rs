use blockchain_gateway_core::chain;

use crate::{GatewayResult, GatewayService};

pub async fn view_account(
    service: &GatewayService,
    params: chain::ViewAccountParams,
) -> GatewayResult<chain::ViewAccountResult> {
    service.near().chain().view_account(params).await
}

pub async fn view_function(
    service: &GatewayService,
    params: chain::ViewFunctionParams,
) -> GatewayResult<chain::ViewFunctionResult> {
    service.near().chain().view_function(params).await
}

pub async fn get_transaction(
    service: &GatewayService,
    params: chain::GetTransactionParams,
) -> GatewayResult<chain::GetTransactionResult> {
    service.near().chain().get_transaction(params).await
}
