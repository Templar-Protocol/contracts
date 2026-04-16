use blockchain_gateway_core::tx;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearClient};

use super::DispatchRead;
use crate::actor::RpcMessage;

impl DispatchRead for tx::Get {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.chain().get_transaction(params.0.params).await })
    }
}
