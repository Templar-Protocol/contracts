use blockchain_gateway_core::{
    operation::{
        OperationId, OperationOutcome, OperationRecord, OperationStatus, StepStatus,
        TransactionStepRecord,
    },
    rpc::common::{WriteOperationResult, WriteRequest},
    tx,
};
use futures::future::BoxFuture;
use near_api::types::transaction::result::TransactionResult;
use uuid::Uuid;

use crate::GatewayService;

pub(crate) fn operation_outcome_from_transaction_result(
    signer_account_id: blockchain_gateway_core::ManagedAccountId,
    tx_result: TransactionResult,
) -> WriteOperationResult {
    let (status, step_status, tx_hash) = if let Some(full) = tx_result.into_full() {
        let outcome = full.outcome();
        let tx_hash = Some(outcome.transaction_hash.to_string());
        let step_status = if full.is_success() {
            StepStatus::Succeeded
        } else {
            StepStatus::Failed
        };
        let status = if full.is_success() {
            OperationStatus::Succeeded
        } else {
            OperationStatus::Failed
        };
        (status, step_status, tx_hash)
    } else {
        (OperationStatus::InProgress, StepStatus::Submitted, None)
    };

    WriteOperationResult {
        outcome: OperationOutcome {
            operation: OperationRecord {
                id: OperationId(
                    tx_hash
                        .clone()
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                ),
                signer_account_id,
                status,
                steps: vec![TransactionStepRecord {
                    index: 0,
                    status: step_status,
                    tx_hash,
                }],
            },
        },
    }
}

pub fn function_call(
    service: &GatewayService,
    request: WriteRequest<tx::FunctionCallBody>,
) -> BoxFuture<'_, crate::GatewayResult<tx::FunctionCallResult>> {
    Box::pin(async move {
        let signer_account_id = request.signer_account_id.clone();
        let tx_result = service
            .writer()
            .tx(request.signer_account_id.clone())?
            .function_call(request.body, request.wait_until)
            .await?;

        Ok(operation_outcome_from_transaction_result(
            signer_account_id,
            tx_result,
        ))
    })
}
