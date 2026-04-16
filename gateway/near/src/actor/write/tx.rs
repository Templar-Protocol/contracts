use blockchain_gateway_core::tx;
use futures::future::BoxFuture;

use crate::{GatewayResult, NearClient};

use super::{operation_outcome_from_transaction_result, WriteRpcRequest};

impl WriteRpcRequest for tx::FunctionCall {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: std::sync::Arc<near_api::Signer>,
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

impl WriteRpcRequest for tx::TransferNep141 {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: std::sync::Arc<near_api::Signer>,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let signer_account_id = request.signer_account_id.clone();
            let body = request.body;
            let tx_result = client
                .tx(request.signer_account_id, signer)
                .function_call(
                    tx::FunctionCallBody {
                        receiver_id: body.token_id,
                        method_name: blockchain_gateway_core::ContractMethodName(
                            "ft_transfer".to_owned(),
                        ),
                        args: blockchain_gateway_core::common::ContractArgs::Json(
                            serde_json::json!({
                                "receiver_id": body.receiver_id,
                                "amount": body.amount.0.to_string(),
                            }),
                        ),
                        gas: blockchain_gateway_core::NearGas::from_tgas(100),
                        deposit: blockchain_gateway_core::NearToken::from_yoctonear(1),
                    },
                    request.wait_until,
                )
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
