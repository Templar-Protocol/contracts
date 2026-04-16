use blockchain_gateway_core::{
    rpc::common::ContractArgs, storage, tx, ContractMethodName, NearGas,
};
use futures::future::BoxFuture;

use crate::{GatewayResult, NearClient};

use super::{operation_outcome_from_transaction_result, WriteRpcRequest};

impl WriteRpcRequest for storage::Deposit {
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
                        receiver_id: body.contract_id,
                        method_name: ContractMethodName("storage_deposit".to_owned()),
                        args: ContractArgs::Json(serde_json::json!({
                            "account_id": body.beneficiary_id,
                            "registration_only": body.registration_only,
                        })),
                        gas: NearGas::from_tgas(100),
                        deposit: body.deposit,
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

impl WriteRpcRequest for storage::Unregister {
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
                        receiver_id: body.contract_id,
                        method_name: ContractMethodName("storage_unregister".to_owned()),
                        args: ContractArgs::Json(serde_json::json!({
                            "force": body.force,
                        })),
                        gas: NearGas::from_tgas(100),
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
