use blockchain_gateway_core::storage;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearReadClient};

use super::ReadRpcRequest;
use crate::actor::rpc::RpcMessage;

impl ReadRpcRequest for storage::GetBalanceBounds {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .storage(params.contract_id)
                .storage_balance_bounds(params.args)
                .await
                .map(|bounds| storage::GetBalanceBoundsResult {
                    bounds: blockchain_gateway_core::common::StorageBalanceBounds {
                        min: bounds.min,
                        max: bounds.max,
                    },
                })
        })
    }
}

impl ReadRpcRequest for storage::GetBalanceOf {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearReadClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.body;
            client
                .storage(params.contract_id)
                .storage_balance_of(params.args)
                .await
                .map(|balance| storage::GetBalanceOfResult {
                    balance: balance.map(|balance| blockchain_gateway_core::common::StorageBalance {
                        total: balance.total,
                        available: balance.available,
                    }),
                })
        })
    }
}
