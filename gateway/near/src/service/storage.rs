use blockchain_gateway_core::{
    rpc::common::WriteRequest, storage, tx, ContractMethodName, NearGas,
};
use futures::future::BoxFuture;

use crate::GatewayService;

use super::tx::operation_outcome_from_transaction_result;

pub fn get_balance_bounds(
    service: &GatewayService,
    params: storage::GetBalanceBoundsParams,
) -> BoxFuture<'_, crate::GatewayResult<storage::GetBalanceBoundsResult>> {
    Box::pin(async move {
        let bounds = service
            .near()
            .storage(params.contract_id)
            .storage_balance_bounds(params.args)
            .await?;

        Ok(storage::GetBalanceBoundsResult {
            bounds: blockchain_gateway_core::common::StorageBalanceBounds {
                min: bounds.min,
                max: bounds.max,
            },
        })
    })
}

pub fn get_balance_of(
    service: &GatewayService,
    params: storage::GetBalanceOfParams,
) -> BoxFuture<'_, crate::GatewayResult<storage::GetBalanceOfResult>> {
    Box::pin(async move {
        let balance = service
            .near()
            .storage(params.contract_id)
            .storage_balance_of(params.args)
            .await?
            .map(|balance| blockchain_gateway_core::common::StorageBalance {
                total: balance.total,
                available: balance.available,
            });

        Ok(storage::GetBalanceOfResult { balance })
    })
}

pub fn deposit(
    service: &GatewayService,
    request: WriteRequest<storage::DepositBody>,
) -> BoxFuture<'_, crate::GatewayResult<storage::DepositResult>> {
    Box::pin(async move {
        let signer_account_id = request.signer_account_id.clone();
        let body = request.body;
        let tx_result = service
            .writer()
            .tx(request.signer_account_id)?
            .function_call(
                tx::FunctionCallBody {
                    receiver_id: body.contract_id,
                    method_name: ContractMethodName("storage_deposit".to_owned()),
                    args: blockchain_gateway_core::common::ContractArgs::Json(serde_json::json!({
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
