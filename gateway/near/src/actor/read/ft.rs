use blockchain_gateway_core::ft;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearClient};

use crate::client::ft::GetBalanceOfArgs;

use super::DispatchRead;
use crate::actor::RpcMessage;

impl DispatchRead for ft::GetBalanceOf {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            let balance = client
                .ft(params.token_id)
                .ft_balance_of(GetBalanceOfArgs {
                    account_id: params.account_id,
                })
                .await?;

            Ok(ft::GetBalanceOfResult { balance })
        })
    }
}
