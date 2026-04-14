use blockchain_gateway_core::{rpc::common::WriteRequest, tx};
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn function_call(
    service: &GatewayService,
    request: WriteRequest<tx::FunctionCallBody>,
) -> BoxFuture<'_, crate::GatewayResult<tx::FunctionCallResult>> {
    Box::pin(async move { service.write().request(request).await })
}
