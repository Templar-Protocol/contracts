use serde::Serialize;
use sha2::{Digest, Sha256};
use templar_gateway_types::{
    common::WriteOperationResult, operation::OperationRecord, operation::OperationStatus,
    IdempotencyKey, OperationId,
};

use crate::{
    CreateOperationResult, CurrentStep, CurrentStepRef, GatewayResult, HasIdempotencyKey,
    HasSignerAccountId, OperationPlan, SharedExecuteOperation, SharedOperationStore,
    SharedSignTransaction, StoredOperation, SucceededStep,
};

#[derive(Clone)]
pub struct OperationDriver {
    store: SharedOperationStore,
    transaction_signer: SharedSignTransaction,
    operation_executor: SharedExecuteOperation,
}

impl OperationDriver {
    pub fn new(
        store: SharedOperationStore,
        transaction_signer: SharedSignTransaction,
        operation_executor: SharedExecuteOperation,
    ) -> Self {
        Self {
            store,
            transaction_signer,
            operation_executor,
        }
    }

    pub async fn get_operation(
        &self,
        operation_id: &OperationId,
    ) -> GatewayResult<Option<OperationRecord>> {
        Ok(self
            .store
            .get_by_id(operation_id)
            .await?
            .map(|operation| operation.operation_record()))
    }

    pub async fn complete_write<Input>(
        &self,
        rpc_method: &'static str,
        params: Input,
        plan: OperationPlan,
    ) -> GatewayResult<WriteOperationResult>
    where
        Input: Clone + Serialize + HasIdempotencyKey + HasSignerAccountId,
    {
        let request_payload = serde_json::to_vec(&params)?;
        let fingerprint = request_fingerprint(rpc_method, &params)?;
        let operation = match self
            .store
            .create_or_get_operation(
                rpc_method,
                params.signer_account_id().to_owned(),
                params.idempotency_key().cloned(),
                fingerprint,
                request_payload,
                plan,
            )
            .await?
        {
            CreateOperationResult::Existing(existing) => {
                return Ok(operation_result_from_stored(existing));
            }
            CreateOperationResult::Created(created) => created,
        };

        let operation = self.execute_remaining_steps(operation).await?;
        Ok(operation_result_from_stored(operation))
    }

    pub async fn create_planned_operation<Input>(
        &self,
        rpc_method: &'static str,
        params: &Input,
        plan: OperationPlan,
    ) -> GatewayResult<CreateOperationResult>
    where
        Input: Serialize + HasIdempotencyKey + HasSignerAccountId,
    {
        let request_payload = serde_json::to_vec(params)?;
        let fingerprint = request_fingerprint(rpc_method, params)?;
        self.store
            .create_or_get_operation(
                rpc_method,
                params.signer_account_id().to_owned(),
                params.idempotency_key().cloned(),
                fingerprint,
                request_payload,
                plan,
            )
            .await
    }

    pub async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>> {
        self.store.get_by_idempotency_key(idempotency_key).await
    }

    pub async fn resume_incomplete_operations(&self) -> GatewayResult<()> {
        for mut operation in self.store.list_incomplete_operations().await? {
            if matches!(operation.current_step, Some(CurrentStep::Submitted { .. })) {
                self.reconcile_submitted_step(&mut operation).await;
                self.store.save_operation(operation).await?;
                continue;
            }

            let _ = self.execute_remaining_steps(operation).await;
        }
        Ok(())
    }

    pub async fn execute_remaining_steps(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        while matches!(
            operation.status(),
            OperationStatus::Pending | OperationStatus::InProgress
        ) {
            operation = self.execute_next_step(operation).await?;
            if operation.status() == OperationStatus::Failed {
                break;
            }
        }
        Ok(operation)
    }

    pub async fn submit_next_step_unchecked(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        if let Some(pending) = operation.begin_next_preparation(self.store.clone()) {
            let prepared = self
                .transaction_signer
                .sign_transaction(pending.transaction().clone())
                .await?;
            pending.finish(prepared).await?;
        }

        if let Some(CurrentStepRef::Prepared(prepared_step)) = operation.current(self.store.clone())
        {
            let wait_until = prepared_step.wait_until();
            let (signed_transaction, submitted_step) = prepared_step.submit().await?;
            let _ = self
                .operation_executor
                .submit_transaction(signed_transaction, wait_until)
                .await?;
            drop(submitted_step);
        }

        Ok(operation)
    }

    async fn execute_next_step(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        if operation.current_step_is_failed() {
            return Ok(operation);
        }

        if let Some(pending) = operation.begin_next_preparation(self.store.clone()) {
            let prepared = self
                .transaction_signer
                .sign_transaction(pending.transaction().clone())
                .await?;
            pending.finish(prepared).await?;
        }

        match operation.current(self.store.clone()) {
            Some(CurrentStepRef::Prepared(prepared_step)) => {
                let wait_until = prepared_step.wait_until();
                let (signed_transaction, submitted_step) = prepared_step.submit().await?;
                let tx_hash = submitted_step.tx_hash();

                match self
                    .operation_executor
                    .submit_transaction(signed_transaction, wait_until)
                    .await
                {
                    Ok(tx_result) => {
                        if let Some(full) = tx_result.into_full() {
                            let final_hash = full.outcome().transaction_hash.into();
                            if full.is_success() {
                                submitted_step.succeed(final_hash).await?;
                            } else {
                                submitted_step.fail(Some(final_hash)).await?;
                            }
                        }
                    }
                    Err(error) => {
                        submitted_step.fail(Some(tx_hash)).await?;
                        return Err(error);
                    }
                }
            }
            Some(CurrentStepRef::Submitted(submitted_step)) => {
                let tx_hash = submitted_step.tx_hash();
                submitted_step.fail(Some(tx_hash)).await?;
            }
            Some(CurrentStepRef::Failed) | None => {}
        }

        Ok(operation)
    }

    async fn reconcile_submitted_step(&self, operation: &mut StoredOperation) {
        if let Some(CurrentStep::Submitted {
            transaction,
            tx_hash,
        }) = operation.current_step.take()
        {
            match self
                .operation_executor
                .query_transaction(&transaction.signer_account_id, tx_hash)
                .await
            {
                Ok(execution) if execution.is_success() => {
                    operation.succeeded_steps.push(SucceededStep {
                        transaction,
                        tx_hash,
                    });
                    operation.current_step = None;
                }
                Ok(_) | Err(_) => {
                    operation.current_step = Some(CurrentStep::Failed {
                        transaction,
                        tx_hash: Some(tx_hash),
                    });
                }
            }
        }
    }
}

fn operation_result_from_stored(operation: StoredOperation) -> WriteOperationResult {
    WriteOperationResult {
        operation: operation.operation_record(),
    }
}

pub fn request_fingerprint<T: Serialize>(method: &str, params: &T) -> GatewayResult<[u8; 32]> {
    let payload = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "method": method,
        "params": params,
    }))?;
    Ok(Sha256::digest(payload).into())
}
