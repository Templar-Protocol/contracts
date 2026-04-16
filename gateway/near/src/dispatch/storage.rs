use std::sync::Arc;

use blockchain_gateway_core::{storage, tx, ContractMethodName};
use futures::future::BoxFuture;

use crate::{
    actor::{operation_outcome_from_transaction_result, DispatchRead, DispatchWrite, RpcMessage},
    GatewayResult, NearClient,
};

impl DispatchRead for storage::GetBalanceBounds {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
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

impl DispatchRead for storage::GetBalanceOf {
    fn dispatch(
        params: RpcMessage<Self>,
        client: NearClient,
    ) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let params = params.0.params;
            client
                .storage(params.contract_id)
                .storage_balance_of(params.args)
                .await
                .map(|balance| storage::GetBalanceOfResult {
                    balance: balance.map(|balance| {
                        blockchain_gateway_core::common::StorageBalance {
                            total: balance.total,
                            available: balance.available,
                        }
                    }),
                })
        })
    }
}

impl DispatchWrite for storage::Deposit {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
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
                        args: blockchain_gateway_core::common::ContractArgs::Json(
                            serde_json::json!({
                                "account_id": body.beneficiary_id,
                                "registration_only": body.registration_only,
                            }),
                        ),
                        gas: blockchain_gateway_core::NearGas::from_tgas(100),
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

impl DispatchWrite for storage::Unregister {
    fn dispatch(
        request: Self::Input,
        client: NearClient,
        signer: Arc<near_api::Signer>,
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
                        args: blockchain_gateway_core::common::ContractArgs::Json(
                            serde_json::json!({
                                "force": body.force,
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
