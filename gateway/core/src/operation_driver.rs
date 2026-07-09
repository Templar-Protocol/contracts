use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    operation::{OperationRecord, OperationStatus},
    IdempotencyKey, MethodSpec, OperationId,
};

use crate::{
    CreateOperationResult, CurrentStep, CurrentStepRef, GatewayError, GatewayResult,
    HasIdempotencyKey, HasSignerAccountId, OperationPlan, PlanWrite, SharedExecuteOperation,
    SharedOperationStore, SharedSignTransaction, StoredOperation,
};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Default)]
struct OperationLocks {
    locks: Mutex<HashMap<OperationId, Arc<Mutex<()>>>>,
}

impl OperationLocks {
    async fn lock(&self, operation_id: &OperationId) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(operation_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }
}

#[derive(Clone)]
pub struct OperationDriver {
    store: SharedOperationStore,
    transaction_signer: SharedSignTransaction,
    operation_executor: SharedExecuteOperation,
    operation_locks: Arc<OperationLocks>,
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
            operation_locks: Arc::new(OperationLocks::default()),
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

    /// Plan a write operation via `Impl` and execute it through this driver
    /// (idempotency, multi-step finalization, replay).
    ///
    /// The operation is *reserved* in the store — recording its idempotency key
    /// and identity — before planning runs, so a slow or crashed planning step
    /// can't leave the durable key→operation link invisible. Recovery and
    /// downstream accounting then see a reserved operation and defer, rather
    /// than treating an in-flight request as one that never reached the gateway.
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
        let rpc_method = Spec::RPC_METHOD;
        let request_payload = serde_json::to_vec(&request)?;
        let fingerprint = request_fingerprint(rpc_method, &request)?;

        // Reserve before planning: the reservation carries no steps yet.
        let operation = match self
            .store
            .create_or_get_operation(
                rpc_method,
                request.signer_account_id().to_owned(),
                request.idempotency_key().cloned(),
                fingerprint,
                request_payload,
                OperationPlan { steps: vec![] },
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
            CreateOperationResult::Created(reserved) => {
                tracing::debug!(
                    rpc_method,
                    operation_id = %reserved.operation_id().0,
                    "reserved gateway operation"
                );
                reserved
            }
        };

        // Plan. On failure, drop the reservation: planning is read-only, so
        // nothing was submitted and any charge held for it is safe to release.
        let plan = match <Impl as PlanWrite<Spec, Ctx>>::plan(request, context).await {
            Ok(plan) => plan,
            Err(error) => {
                if let Err(cleanup) = self
                    .store
                    .delete_reservation(operation.operation_id())
                    .await
                {
                    tracing::warn!(
                        operation_id = %operation.operation_id().0,
                        %cleanup,
                        "failed to remove reservation after planning error"
                    );
                }
                return Err(error);
            }
        };

        self.finish_reserved(operation, plan).await
    }

    /// Attach a plan to a reserved operation and run it to a terminal outcome.
    /// Planning has completed, so the reservation is promoted to a real
    /// operation — including for an empty (no-op) plan, which becomes a terminal
    /// `Succeeded` operation with no steps so the idempotency key keeps resolving
    /// to that result on retry (rather than re-planning a possibly-changed world).
    async fn finish_reserved(
        &self,
        operation: StoredOperation,
        plan: OperationPlan,
    ) -> GatewayResult<WriteOperationResult> {
        let operation_id = operation.operation_id().clone();
        let _operation_guard = self.operation_locks.lock(&operation_id).await;
        let mut operation = self
            .store
            .get_by_id(&operation_id)
            .await?
            .unwrap_or(operation);

        if !operation.is_reservation() {
            return Ok(operation.record().into());
        }

        operation.planned = true;

        if plan.steps.is_empty() {
            // No-op: persist the terminal, step-less operation for idempotency.
            // Best-effort like the terminal save below — the operation already
            // reached its (Succeeded, no-action) outcome, so a transient store
            // failure must not error the caller.
            if let Err(error) = self.store.save_operation(operation.clone()).await {
                tracing::warn!(
                    operation_id = %operation.operation_id().0,
                    %error,
                    "failed to persist terminal no-op operation state for store book-keeping"
                );
            }
            return Ok(operation.record().into());
        }

        // Attach the plan and execute. Each step persists the full remaining set
        // before its first irreversible (on-chain) action, so recovery can
        // resume a partially-run multi-step operation.
        operation.remaining_steps = VecDeque::from(plan.steps);
        let operation = match self.execute_remaining_steps_unlocked(operation).await {
            Ok(operation) => operation,
            Err(error) => {
                // If execution failed before persisting any step (e.g. a presign
                // error), nothing was submitted and the store still holds only
                // the bare reservation. Drop it so accounting can release the
                // charge now instead of waiting for startup recovery. Log a failed
                // cleanup (like the planning-error path) so a leaked reservation is
                // observable rather than lingering silently until the next sweep.
                match self.store.get_by_id(&operation_id).await {
                    Ok(Some(stored)) if stored.is_reservation() => {
                        if let Err(cleanup) = self.store.delete_reservation(&operation_id).await {
                            tracing::warn!(
                                operation_id = %operation_id.0,
                                %cleanup,
                                "failed to remove reservation after execution error"
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(lookup) => tracing::warn!(
                        operation_id = %operation_id.0,
                        %lookup,
                        "failed to look up operation for reservation cleanup after execution error"
                    ),
                }
                return Err(error);
            }
        };
        // Best-effort terminal save: the operation already reached its outcome,
        // so a transient store failure here must not error the caller.
        if let Err(error) = self.store.save_operation(operation.clone()).await {
            tracing::warn!(
                operation_id = %operation.operation_id().0,
                %error,
                "failed to persist terminal operation state for store book-keeping"
            );
        }
        Ok(operation.record().into())
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

    /// Drive every incomplete operation toward terminal, best-effort. Failures to
    /// advance individual operations are logged and counted; if any fail, the whole
    /// pass returns an error so a caller (e.g. the relayer's fatal startup-recovery
    /// gate) can fail fast rather than serve with charges recovery could not
    /// resolve. Every operation is still attempted first, so a single wedged one
    /// does not block recovering the rest.
    pub async fn resume_incomplete_operations(&self) -> GatewayResult<()> {
        let operations = self.store.list_incomplete_operations().await?;
        tracing::debug!(
            operation_count = operations.len(),
            "resuming incomplete gateway operations"
        );
        let mut failures = 0_usize;
        for operation in operations {
            let operation_id = operation.operation_id().clone();
            let _operation_guard = self.operation_locks.lock(&operation_id).await;
            let Some(operation) = self.store.get_by_id(&operation_id).await? else {
                continue;
            };

            // Recovery runs before this process serves traffic, so any reservation
            // is from a previous run that died mid-planning: planning is read-only,
            // nothing was submitted, so drop it and release its charge. (This is
            // the only place reservations are reaped — a live one being planned is
            // never visible here.)
            if operation.is_reservation() {
                if let Err(error) = self.store.delete_reservation(&operation_id).await {
                    failures += 1;
                    tracing::warn!(
                        operation_id = %operation_id.0,
                        %error,
                        "failed to reap reservation during startup recovery"
                    );
                }
                continue;
            }

            if let Err(error) = self.advance_operation_unlocked(operation).await {
                failures += 1;
                tracing::warn!(
                    operation_id = %operation_id.0,
                    %error,
                    "failed to resume incomplete gateway operation"
                );
            }
        }
        if failures > 0 {
            return Err(GatewayError::IncompleteRecovery(format!(
                "{failures} operation(s) failed to resume"
            )));
        }
        Ok(())
    }

    /// Drive an in-flight operation toward a terminal outcome and persist it:
    /// reconcile a submitted step against the chain (if any), then run the
    /// remaining steps. Shared by startup recovery, the broom's reconcile, and
    /// idempotent retries, so all resolve a submitted step *and* continue a
    /// multi-step plan.
    async fn advance_operation_unlocked(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        if matches!(operation.current_step, Some(CurrentStep::Submitted { .. })) {
            self.reconcile_submitted_step(&mut operation).await;
        }
        let operation = self.execute_remaining_steps_unlocked(operation).await?;
        self.store.save_operation(operation.clone()).await?;
        Ok(operation)
    }

    pub async fn execute_remaining_steps(
        &self,
        operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        let operation_id = operation.operation_id().clone();
        let _operation_guard = self.operation_locks.lock(&operation_id).await;
        let operation = self
            .store
            .get_by_id(&operation_id)
            .await?
            .unwrap_or(operation);
        self.execute_remaining_steps_unlocked(operation).await
    }

    async fn execute_remaining_steps_unlocked(
        &self,
        mut operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        while matches!(
            operation.status(),
            OperationStatus::Pending | OperationStatus::InProgress
        ) {
            operation = self.execute_next_step(operation).await?;
            // Stop once a step is terminal-failed, or in flight (Submitted with no
            // captured outcome) — the latter can only be resolved by reconciling
            // against the chain, not by driving more steps.
            if operation.status() == OperationStatus::Failed
                || matches!(operation.current_step, Some(CurrentStep::Submitted { .. }))
            {
                break;
            }
        }
        Ok(operation)
    }

    pub async fn submit_next_step_unchecked(
        &self,
        operation: StoredOperation,
    ) -> GatewayResult<StoredOperation> {
        let operation_id = operation.operation_id().clone();
        let _operation_guard = self.operation_locks.lock(&operation_id).await;
        let mut operation = self
            .store
            .get_by_id(&operation_id)
            .await?
            .unwrap_or(operation);

        if let Some(pending) = operation.begin_next_preparation(self.store.clone()) {
            let prepared = self
                .transaction_signer
                .sign_transaction(pending.transaction().clone())
                .await?;
            pending.finish(prepared).await?;
        }

        if let Some(CurrentStepRef::Prepared(prepared_step)) = operation.current(self.store.clone())
        {
            let (signed_transaction, submitted_step) = prepared_step.submit().await?;
            let _ = self
                .operation_executor
                .submit_transaction(signed_transaction)
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
                let (signed_transaction, submitted_step) = prepared_step.submit().await?;
                let tx_hash = submitted_step.tx_hash();
                tracing::debug!(
                    operation_id = %operation_id.0,
                    %tx_hash,
                    "submitting gateway operation step"
                );

                match self
                    .operation_executor
                    .submit_transaction(signed_transaction)
                    .await
                {
                    // The submission already carries the full execution outcome,
                    // so callers needn't re-fetch it with a separate `tx.get`.
                    Ok(Some(step)) => {
                        if step.is_success {
                            tracing::debug!(
                                operation_id = %operation_id.0,
                                tx_hash = %step.tx_hash,
                                "gateway operation step succeeded"
                            );
                            submitted_step
                                .mark_succeeded(step.tx_hash, step.outcome)
                                .await?;
                        } else {
                            tracing::debug!(
                                operation_id = %operation_id.0,
                                tx_hash = %step.tx_hash,
                                "gateway operation step reverted"
                            );
                            submitted_step
                                .mark_reverted(step.tx_hash, step.outcome)
                                .await?;
                        }
                    }
                    // Broadcast but no full outcome yet: leave it Submitted for
                    // reconciliation to resolve by querying the chain.
                    Ok(None) => {}
                    Err(error) => {
                        // The transaction may have reached the chain before this
                        // error (e.g. an RPC timeout after broadcast). Leave the
                        // step Submitted and let reconciliation settle its fate by
                        // querying the chain; rejecting here could release a charge
                        // for a transaction that landed.
                        tracing::warn!(
                            operation_id = %operation_id.0,
                            %tx_hash,
                            %error,
                            "gateway operation step submission failed; left submitted for reconciliation"
                        );
                        return Err(error);
                    }
                }
            }
            // A step left Submitted (no full outcome at submit time) is in flight:
            // only reconciliation, querying by hash, can resolve it. The driving
            // loop stops on a Submitted step, so this is reached only defensively.
            Some(CurrentStepRef::Submitted(_) | CurrentStepRef::Failed) | None => {}
        }

        Ok(operation)
    }

    /// Reconcile a single in-flight operation (looked up by idempotency key)
    /// against the chain and return its current record — used by the relayer's
    /// broom to drive a stuck operation toward terminal without waiting for the
    /// next startup.
    ///
    /// Returns `None` when there is no such operation. A reservation is left
    /// untouched (its owning request is still planning it); only startup recovery
    /// reaps reservations.
    pub async fn reconcile_operation(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<OperationRecord>> {
        let Some(operation) = self.store.get_by_idempotency_key(idempotency_key).await? else {
            return Ok(None);
        };
        let operation_id = operation.operation_id().clone();
        let _operation_guard = self.operation_locks.lock(&operation_id).await;
        let Some(operation) = self.store.get_by_idempotency_key(idempotency_key).await? else {
            return Ok(None);
        };
        if operation.is_reservation() {
            return Ok(Some(operation.record()));
        }
        Ok(Some(
            self.advance_operation_unlocked(operation).await?.record(),
        ))
    }

    /// Query a submitted step's on-chain fate and record it. The ambiguous case —
    /// a transaction the chain has no record of — is left submitted because NEAR
    /// `UNKNOWN_TRANSACTION` is not proof that the transaction never landed.
    async fn reconcile_submitted_step(&self, operation: &mut StoredOperation) {
        if let Some(CurrentStep::Submitted {
            transaction,
            tx_hash,
            submitted_at,
        }) = operation.current_step.take()
        {
            match self
                .operation_executor
                .query_transaction(&transaction.signer_account_id, tx_hash)
                .await
            {
                Ok(step) => {
                    if step.is_success {
                        operation.record_success(transaction, tx_hash, step.outcome);
                    } else {
                        tracing::warn!(
                            operation_id = %operation.id.0,
                            %tx_hash,
                            "current step transaction reverted"
                        );
                        // Tolerance is decided by the step's own flag, so a
                        // continue_on_failure step reverting mid-reconcile advances
                        // rather than blocking — same rule as live execution.
                        operation.record_revert(transaction, tx_hash, step.outcome);
                    }
                }
                Err(error) => {
                    // A transient failure or unknown transaction may still have
                    // executed, so do NOT reject. Leave it submitted to reconcile
                    // on a later sweep.
                    tracing::warn!(
                        operation_id = %operation.id.0,
                        %tx_hash,
                        %error,
                        "leaving submitted gateway transaction for a later reconciliation"
                    );
                    operation.current_step = Some(CurrentStep::Submitted {
                        transaction,
                        tx_hash,
                        submitted_at,
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
