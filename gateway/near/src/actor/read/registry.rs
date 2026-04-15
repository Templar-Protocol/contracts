use blockchain_gateway_core::registry;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearReadClient};

use super::ReadRpcRequest;
use crate::actor::rpc::RpcMessage;

impl ReadRpcRequest for registry::ListDeployments {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .registry(params.registry_id)
                .list_deployments(params.args)
                .await
                .map(|account_ids| registry::ListDeploymentsResult { account_ids })
        })
    }
}

impl ReadRpcRequest for registry::ListVersions {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .registry(params.registry_id)
                .list_versions(params.args)
                .await
                .map(|values| registry::ListVersionsResult { values })
        })
    }
}
