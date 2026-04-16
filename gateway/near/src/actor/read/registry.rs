use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::{client::registry::GetDeploymentArgs, GatewayResult, NearClient};

use super::DispatchRead;
use crate::actor::RpcMessage;

impl DispatchRead for registry::ListDeployments {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .registry(params.registry_id)
                .list_deployments(params.args)
                .await
                .map(|account_ids| registry::ListDeploymentsResult { account_ids })
        })
    }
}

impl DispatchRead for registry::GetDeployment {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .registry(params.registry_id)
                .get_deployment(GetDeploymentArgs {
                    account_id: params.account_id,
                })
                .await
                .map(|deployment| registry::GetDeploymentResult { deployment })
        })
    }
}

impl DispatchRead for registry::ListVersions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .registry(params.registry_id)
                .list_versions(params.args)
                .await
                .map(|values| registry::ListVersionsResult { values })
        })
    }
}
