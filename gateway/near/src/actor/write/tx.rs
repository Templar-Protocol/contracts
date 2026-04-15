use blockchain_gateway_core::{
    tx,
};
use futures::future::BoxFuture;

use crate::{GatewayResult, NearWriteClient};

use super::{WriteRpcRequest, operation_outcome_from_transaction_result};

impl WriteRpcRequest for tx::FunctionCall {
    fn dispatch(
        request: Self::Input,
        client: NearWriteClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .tx(request.signer_account_id.clone())?
                .function_call(request.body, request.wait_until)
                .await?;

            Ok(operation_outcome_from_transaction_result(
                signer_account_id,
                tx_result,
            ))
        })
    }

    fn signer_account_id(request: &Self::Input) -> &blockchain_gateway_core::ManagedAccountId {
        &request.signer_account_id
    }
}
