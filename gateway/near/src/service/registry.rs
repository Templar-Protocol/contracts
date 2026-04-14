use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn list_deployments(
    service: &GatewayService,
    params: registry::ListDeploymentsParams,
) -> BoxFuture<'_, crate::GatewayResult<registry::ListDeploymentsResult>> {
    Box::pin(async move { service.read().request(params).await })
}

pub fn list_versions(
    service: &GatewayService,
    params: registry::ListVersionsParams,
) -> BoxFuture<'_, crate::GatewayResult<registry::ListVersionsResult>> {
    Box::pin(async move { service.read().request(params).await })
}
