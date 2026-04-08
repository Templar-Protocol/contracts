use blockchain_gateway_core::registry;

use crate::{GatewayResult, GatewayService};

pub async fn list_deployments(
    service: &GatewayService,
    params: registry::ListDeploymentsParams,
) -> GatewayResult<registry::ListDeploymentsResult> {
    let account_ids = service
        .near()
        .registry(params.registry_id)
        .list_deployments(params.args)
        .await?;

    Ok(registry::ListDeploymentsResult { account_ids })
}

pub async fn list_versions(
    service: &GatewayService,
    params: registry::ListVersionsParams,
) -> GatewayResult<registry::ListVersionsResult> {
    let values = service
        .near()
        .registry(params.registry_id)
        .list_versions(params.args)
        .await?;

    Ok(registry::ListVersionsResult { values })
}
