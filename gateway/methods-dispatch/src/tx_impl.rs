use async_trait::async_trait;
use near_api::types::transaction::actions::{
    Action, DeployContractAction, FunctionCallAction, TransferAction,
};
use templar_gateway_core::{
    DispatchRead, GatewayResult, HasNearClient, OperationPlan, PlanWrite, PlannedTransaction,
};
use templar_gateway_methods_spec::tx;
use templar_gateway_types::MethodSpec;

use crate::Dispatch;

#[async_trait]
impl<C: HasNearClient> DispatchRead<tx::Get, C> for Dispatch {
    async fn dispatch(
        request: <tx::Get as MethodSpec>::Input,
        ctx: C,
    ) -> GatewayResult<tx::GetResult> {
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
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::FunctionCall, C> for Dispatch {
    async fn plan(
        request: <tx::FunctionCall as MethodSpec>::Input,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
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
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::Transfer, C> for Dispatch {
    async fn plan(
        request: <tx::Transfer as MethodSpec>::Input,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::single(PlannedTransaction {
            signer_account_id: request.signer_account_id,
            wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
            receiver_id: request.body.receiver_id,
            actions: vec![Action::Transfer(TransferAction {
                deposit: request.body.amount,
            })],
        }))
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::DeployContract, C> for Dispatch {
    async fn plan(
        request: <tx::DeployContract as MethodSpec>::Input,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan::single(PlannedTransaction {
            signer_account_id: request.signer_account_id,
            wait_until: templar_gateway_types::common::TxExecutionStatus::ExecutedOptimistic,
            receiver_id: request.body.account_id,
            actions: vec![Action::DeployContract(DeployContractAction {
                code: request.body.code.0,
            })],
        }))
    }
}

#[async_trait]
impl<C: Send + 'static> PlanWrite<tx::DeployAndInit, C> for Dispatch {
    async fn plan(
        request: <tx::DeployAndInit as MethodSpec>::Input,
        _context: C,
    ) -> GatewayResult<OperationPlan> {
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
    }
}
