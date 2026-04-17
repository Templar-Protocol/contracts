use blockchain_gateway_core::contract;
use futures::future::BoxFuture;

use crate::{
    actor::{DispatchRead, RpcMessage},
    ops, GatewayResult, NearClient,
};

impl DispatchRead for contract::ViewFunction {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            client
                .contract(params.0.params.contract_id.clone())
                .view_function(params.0.params)
                .await
        })
    }
}

impl DispatchRead for contract::GetVersion {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move { ops::contract::get_version(&client, params.0.params).await })
    }
}
