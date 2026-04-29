use futures::future::BoxFuture;
use near_api::types::transaction::actions::{
    Action, DeployContractAction, FunctionCallAction, TransferAction,
};
use templar_gateway_types::tx;

use crate::{
    operation::{OperationPlan, PlannedTransaction},
    GatewayResult, HasNearClient,
};
use crate::{DispatchRead, PlanWrite};

impl<C: HasNearClient> DispatchRead<C> for tx::Get {
    fn dispatch(request: Self::Input, ctx: C) -> BoxFuture<'static, GatewayResult<Self::Output>> {
        Box::pin(async move {
            let result = ctx
                .near_client()
                .chain()
                .get_transaction(
                    request.params.tx_hash.into(),
                    request.params.sender_account_id,
                    request.params.wait_until.unwrap_or_default().into(),
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
                return_value: match request.params.encoding {
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

impl<C> PlanWrite<C> for tx::FunctionCall {
    fn plan(request: Self::Input, _context: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan::single(PlannedTransaction {
                signer_account_id: request.signer_account_id,
                wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
                receiver_id: request.body.receiver_id,
                actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: request.body.method_name.0,
                    args: request.body.args.try_into_bytes()?,
                    gas: request.body.gas,
                    deposit: request.body.deposit,
                }))],
            }))
        })
    }
}

impl<C> PlanWrite<C> for tx::Transfer {
    fn plan(request: Self::Input, _context: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan::single(PlannedTransaction {
                signer_account_id: request.signer_account_id,
                wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
                receiver_id: request.body.receiver_id,
                actions: vec![Action::Transfer(TransferAction {
                    deposit: request.body.amount,
                })],
            }))
        })
    }
}

impl<C> PlanWrite<C> for tx::DeployContract {
    fn plan(request: Self::Input, _context: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan::single(PlannedTransaction {
                signer_account_id: request.signer_account_id,
                wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
                receiver_id: request.body.account_id,
                actions: vec![Action::DeployContract(DeployContractAction {
                    code: request.body.code.0,
                })],
            }))
        })
    }
}

impl<C> PlanWrite<C> for tx::DeployAndInit {
    fn plan(request: Self::Input, _context: C) -> BoxFuture<'static, GatewayResult<OperationPlan>> {
        Box::pin(async move {
            Ok(OperationPlan::single(PlannedTransaction {
                signer_account_id: request.signer_account_id,
                wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
                receiver_id: request.body.account_id,
                actions: vec![
                    Action::DeployContract(DeployContractAction {
                        code: request.body.code.0,
                    }),
                    Action::FunctionCall(Box::new(FunctionCallAction {
                        method_name: request.body.method_name.0,
                        args: request.body.args.try_into_bytes()?,
                        gas: request.body.gas,
                        deposit: request.body.deposit,
                    })),
                ],
            }))
        })
    }
}
