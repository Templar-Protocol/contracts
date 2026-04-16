use blockchain_gateway_core::account;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearClient};

use super::DispatchRead;
use crate::actor::RpcMessage;

impl DispatchRead for account::Get {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { client.account().get(params.0.params).await })
    }
}
