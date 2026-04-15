use blockchain_gateway_core::chain;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearReadClient};

use super::ReadRpcRequest;
use crate::actor::rpc::RpcMessage;

impl ReadRpcRequest for chain::ViewAccount {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().view_account(params.0.body).await })
    }
}

impl ReadRpcRequest for chain::ViewFunction {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().view_function(params.0.body).await })
    }
}

impl ReadRpcRequest for chain::GetTransaction {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().get_transaction(params.0.body).await })
    }
}
