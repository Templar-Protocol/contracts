use std::sync::Arc;

use blockchain_gateway_core::tx;
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite, RpcMessage},
    GatewayResult, NearClient,
};

impl DispatchRead for tx::Get {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            let result = client
                .chain()
                .get_transaction(
                    params.tx_hash.into(),
                    params.sender_account_id,
                    params.wait_until.unwrap_or_default().into(),
                )
                .await?;

            Ok(tx::GetResult {
                status: if result.is_success() {
                    tx::Status::Succeeded
                } else if result.is_pending() {
                    tx::Status::Pending
                } else {
                    tx::Status::Failed
                },
                total_gas_burnt: result.total_gas_burnt,
                logs: result.logs().into_iter().map(ToString::to_string).collect(),
                return_value: match params.encoding {
                    tx::ValueEncoding::Json => result.json().ok().map(tx::ReturnValue::Json),
                    tx::ValueEncoding::Base64 => result
                        .raw_bytes()
                        .ok()
                        .map(|b| tx::ReturnValue::Base64(b.into())),
                },
            })
        })
    }
}

impl DispatchWrite for tx::FunctionCall {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let tx_result = client
                .tx(request.signer_account_id.clone(), signer)
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
