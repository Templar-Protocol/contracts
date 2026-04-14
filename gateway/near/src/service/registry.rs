use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::GatewayService;

pub fn list_deployments(
    service: &GatewayService,
    params: registry::ListDeploymentsParams,
) -> BoxFuture<'_, crate::GatewayResult<registry::ListDeploymentsResult>> {
    Box::pin(async move {
        let account_ids = service
            .near()
            .registry(params.registry_id)
            .list_deployments(params.args)
            .await?;

        Ok(registry::ListDeploymentsResult { account_ids })
    })
}

pub fn list_versions(
    service: &GatewayService,
    params: registry::ListVersionsParams,
) -> BoxFuture<'_, crate::GatewayResult<registry::ListVersionsResult>> {
    Box::pin(async move {
        let values = service
            .near()
            .registry(params.registry_id)
            .list_versions(params.args)
            .await?;

        Ok(registry::ListVersionsResult { values })
    })
}
