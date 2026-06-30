use serde::Serialize;
use sha2::{Digest, Sha256};
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    operation::{ExecutionOutcome, OperationRecord, OperationStatus},
    IdempotencyKey, MethodSpec, OperationId,
};

use crate::{
    CreateOperationResult, CurrentStep, CurrentStepRef, GatewayResult, HasIdempotencyKey,
    HasSignerAccountId, OperationPlan, PlanWrite, SharedExecuteOperation, SharedOperationStore,
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
            .map(|operation| operation.record()))
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
        let step_count = plan.steps.len();
        tracing::debug!(
            rpc_method,
            signer_account_id = %params.signer_account_id().0,
            step_count,
            "creating or reusing gateway operation"
        );
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
                tracing::debug!(
                    rpc_method,
                    operation_id = %existing.operation_id().0,
                    "reusing existing gateway operation"
                );
                return Ok(existing.record().into());
            }
            CreateOperationResult::Created(created) => {
                tracing::debug!(
                    rpc_method,
                    operation_id = %created.operation_id().0,
                    "created gateway operation"
                );
                created
            }
        };

        let operation = self.execute_remaining_steps(operation).await?;
        // Persist the final (terminal) state through the normal store path. For
        // multi-step operations the last step already saved this, making it a
        // harmless re-save; for a zero-step plan (already terminal at creation,
        // e.g. a no-op `storage.ensureDeposit`) this is the only save, and it is
        // what lets the store account for the completed operation (e.g. bounded
        // retention/eviction in `MemoryStore`).
        //
        // This is best-effort book-keeping: the operation has already reached
        // its terminal outcome, so a transient store failure here must not turn
        // a completed operation into an error for the caller.
        if let Err(error) = self.store.save_operation(operation.clone()).await {
            tracing::warn!(
                operation_id = %operation.operation_id().0,
                %error,
                "failed to persist terminal operation state for store book-keeping"
            );
        }
        Ok(operation.record().into())
    }

    /// Plan a write operation via `Impl` and execute it through this driver
    /// (idempotency, multi-step finalization, replay).
    ///
    /// This is the shared write path behind both the direct client and the RPC
    /// service, so neither re-implements signing/submission.
    pub async fn plan_and_complete<Spec, Impl, Ctx>(
        &self,
        context: Ctx,
        request: WriteRequest<Spec>,
    ) -> GatewayResult<WriteOperationResult>
    where
        Spec: MethodSpec<Output = WriteOperationResult>,
        Impl: PlanWrite<Spec, Ctx>,
    {
        let plan = <Impl as PlanWrite<Spec, Ctx>>::plan(request.clone(), context).await?;
        self.complete_write(Spec::RPC_METHOD, request, plan).await
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
        let operations = self.store.list_incomplete_operations().await?;
        tracing::debug!(
            operation_count = operations.len(),
            "resuming incomplete gateway operations"
        );
        for mut operation in operations {
            let operation_id = operation.operation_id().clone();
            if matches!(operation.current_step, Some(CurrentStep::Submitted { .. })) {
                self.reconcile_submitted_step(&mut operation).await;
                self.store.save_operation(operation).await?;
                continue;
            }

            if let Err(error) = self.execute_remaining_steps(operation).await {
                tracing::warn!(
                    operation_id = %operation_id.0,
                    %error,
                    "failed to resume incomplete gateway operation"
                );
            }
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

        let operation_id = operation.operation_id().clone();
        match operation.current(self.store.clone()) {
            Some(CurrentStepRef::Prepared(prepared_step)) => {
                let wait_until = prepared_step.wait_until();
                let (signed_transaction, submitted_step) = prepared_step.submit().await?;
                let tx_hash = submitted_step.tx_hash();
                tracing::debug!(
                    operation_id = %operation_id.0,
                    %tx_hash,
                    ?wait_until,
                    "submitting gateway operation step"
                );

                match self
                    .operation_executor
                    .submit_transaction(signed_transaction, wait_until)
                    .await
                {
                    Ok(tx_result) => {
                        if let Some(full) = tx_result.into_full() {
                            let final_hash = full.outcome().transaction_hash.into();
                            let is_success = full.is_success();
                            // The submission result already carries the full
                            // execution outcome — capture it so callers needn't
                            // re-fetch it with a separate `tx.get`.
                            let outcome = ExecutionOutcome::from(full);
                            if is_success {
                                tracing::debug!(
                                    operation_id = %operation_id.0,
                                    tx_hash = %final_hash,
                                    "gateway operation step succeeded"
                                );
                                submitted_step.mark_succeeded(final_hash, outcome).await?;
                            } else {
                                tracing::debug!(
                                    operation_id = %operation_id.0,
                                    tx_hash = %final_hash,
                                    "gateway operation step failed"
                                );
                                submitted_step.mark_reverted(final_hash, outcome).await?;
                            }
                        }
                    }
                    Err(error) => {
                        tracing::debug!(
                            operation_id = %operation_id.0,
                            %tx_hash,
                            %error,
                            "gateway operation step submission failed"
                        );
                        submitted_step.mark_rejected(tx_hash).await?;
                        return Err(error);
                    }
                }
            }
            Some(CurrentStepRef::Submitted(submitted_step)) => {
                let tx_hash = submitted_step.tx_hash();
                submitted_step.mark_rejected(tx_hash).await?;
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
                Ok(execution) => {
                    let is_success = execution.is_success();
                    let outcome = ExecutionOutcome::from(execution);
                    if is_success {
                        operation.succeeded_steps.push(SucceededStep {
                            transaction,
                            tx_hash,
                            outcome,
                        });
                        operation.current_step = None;
                    } else {
                        tracing::warn!(
                            operation_id = %operation.id.0,
                            %tx_hash,
                            "current step transaction reverted"
                        );
                        operation.current_step = Some(CurrentStep::Reverted {
                            transaction,
                            tx_hash,
                            outcome,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        operation_id = %operation.id.0,
                        %tx_hash,
                        %error,
                        "failed to reconcile submitted gateway transaction"
                    );
                    operation.current_step = Some(CurrentStep::Rejected {
                        transaction,
                        tx_hash,
                    });
                }
            }
        }
    }
}

pub fn request_fingerprint<T: Serialize>(method: &str, params: &T) -> GatewayResult<[u8; 32]> {
    let payload = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "method": method,
        "params": params,
    }))?;
    Ok(Sha256::digest(payload).into())
}
