use std::{collections::HashMap, sync::Arc};

use actix::Addr;
use blockchain_gateway_core::{
    operation::{OperationStatus, StepStatus},
    rpc::common::WriteOperationResult,
    ManagedAccountId,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{
    actor::{
        map_mailbox_error, DispatchRead, HasIdempotencyKey, HasSignerAccountId, ManagedSigner,
        PlanWrite, ReadActor, RpcMessage, WriteActors,
    },
    operation::StoredOperation,
    store::{MemoryOperationStore, SharedOperationStore},
    GatewayContext, GatewayResult,
};

use super::runtime::{spawn_runtime, GatewayRuntime};

#[derive(Clone)]
pub struct GatewayService {
    inner: Arc<GatewayInner>,
    runtime: Arc<Mutex<Option<GatewayRuntime>>>,
}

struct GatewayInner {
    context: GatewayContext,
    read: Addr<ReadActor>,
    write: WriteActors,
    store: SharedOperationStore,
}

impl GatewayService {
    pub fn spawn(
        context: GatewayContext,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
    ) -> Self {
        let (runtime, read, write) = spawn_runtime(context.clone(), signers);

        let service = Self {
            inner: Arc::new(GatewayInner {
                context,
                read,
                write,
                store: Arc::new(MemoryOperationStore::new()),
            }),
            runtime: Arc::new(Mutex::new(Some(runtime))),
        };

        tokio::spawn({
            let service = service.clone();
            async move {
                let _ = service.resume_incomplete_operations().await;
            }
        });

        service
    }

    pub async fn shutdown(self) {
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.shutdown();
        }
    }

    pub async fn request_read<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: DispatchRead,
        ReadActor: actix::Handler<RpcMessage<Request>>,
    {
        self.inner
            .read
            .send(RpcMessage(params))
            .await
            .map_err(|error| map_mailbox_error(error, "read-actor"))?
    }

    pub async fn request_write<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: PlanWrite,
        Request::Input: Clone + Serialize + HasIdempotencyKey,
    {
        self.request_planned_write::<Request>(params).await
    }

    async fn request_planned_write<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: PlanWrite,
        Request::Input: Clone + Serialize,
    {
        let request_payload = serde_json::to_vec(&params)?;
        let fingerprint = request_fingerprint(Request::RPC_METHOD, &params)?;
        if let Some(idempotency_key) = params.idempotency_key() {
            if let Some(existing) = self
                .inner
                .store
                .get_by_idempotency_key(idempotency_key)
                .await?
            {
                if existing.request_fingerprint_hash != fingerprint {
                    return Err(crate::GatewayError::IdempotencyConflict);
                }
                return Ok(operation_result_from_stored(existing));
            }
        }

        let plan = Request::plan(params.clone(), self.read_context()).await?;
        let mut operation = self
            .inner
            .store
            .create_operation(
                params.signer_account_id().to_owned(),
                params.idempotency_key().cloned(),
                fingerprint,
                request_payload,
                plan,
            )
            .await?;

        operation.operation.status = OperationStatus::InProgress;
        self.inner.store.save_operation(operation.clone()).await?;

        for (index, step) in operation.plan.steps.clone().into_iter().enumerate() {
            let tx_result = self
                .inner
                .write
                .execute_planned_transaction(step, operation.plan.wait_until)
                .await;

            match tx_result {
                Ok(tx_result) => {
                    let record = &mut operation.operation.steps[index];
                    if let Some(full) = tx_result.into_full() {
                        let tx_hash = full.outcome().transaction_hash.into();
                        record.status = if full.is_success() {
                            StepStatus::Succeeded { tx_hash }
                        } else {
                            operation.operation.status = OperationStatus::Failed;
                            StepStatus::Failed {
                                tx_hash: Some(tx_hash),
                            }
                        };
                    } else {
                        record.status = StepStatus::Submitted { tx_hash: None };
                        if operation.operation.status != OperationStatus::Failed {
                            operation.operation.status = OperationStatus::InProgress;
                        }
                    }
                }
                Err(error) => {
                    operation.operation.steps[index].status = StepStatus::Failed { tx_hash: None };
                    operation.operation.status = OperationStatus::Failed;
                    self.inner.store.save_operation(operation.clone()).await?;
                    return Err(error);
                }
            }

            self.inner.store.save_operation(operation.clone()).await?;
            if operation.operation.status == OperationStatus::Failed {
                return Ok(operation_result_from_stored(operation));
            }
        }

        if operation
            .operation
            .steps
            .iter()
            .all(|step| matches!(step.status, StepStatus::Succeeded { .. }))
        {
            operation.operation.status = OperationStatus::Succeeded;
        }
        self.inner.store.save_operation(operation.clone()).await?;
        Ok(operation_result_from_stored(operation))
    }

    async fn resume_incomplete_operations(&self) -> GatewayResult<()> {
        for mut operation in self.inner.store.list_incomplete_operations().await? {
            if operation.operation.steps.iter().any(|step| {
                matches!(
                    step.status,
                    StepStatus::Submitted { .. } | StepStatus::Failed { .. }
                )
            }) {
                operation.operation.status = OperationStatus::Failed;
                for step in &mut operation.operation.steps {
                    if !matches!(step.status, StepStatus::Succeeded { .. }) {
                        let tx_hash = match &step.status {
                            StepStatus::Submitted { tx_hash } | StepStatus::Failed { tx_hash } => {
                                tx_hash.clone()
                            }
                            StepStatus::Succeeded { .. } | StepStatus::NotStarted => None,
                        };
                        step.status = StepStatus::Failed { tx_hash };
                    }
                }
                self.inner.store.save_operation(operation).await?;
                continue;
            }

            operation.operation.status = OperationStatus::InProgress;
            self.inner.store.save_operation(operation.clone()).await?;

            for (index, step) in operation.plan.steps.clone().into_iter().enumerate() {
                if matches!(
                    operation.operation.steps[index].status,
                    StepStatus::Succeeded { .. }
                ) {
                    continue;
                }

                let tx_result = self
                    .inner
                    .write
                    .execute_planned_transaction(step, operation.plan.wait_until)
                    .await;

                match tx_result {
                    Ok(tx_result) => {
                        let record = &mut operation.operation.steps[index];
                        if let Some(full) = tx_result.into_full() {
                            let tx_hash = full.outcome().transaction_hash.into();
                            record.status = if full.is_success() {
                                StepStatus::Succeeded { tx_hash }
                            } else {
                                operation.operation.status = OperationStatus::Failed;
                                StepStatus::Failed {
                                    tx_hash: Some(tx_hash),
                                }
                            };
                        } else {
                            operation.operation.status = OperationStatus::Failed;
                            record.status = StepStatus::Failed { tx_hash: None };
                        }
                    }
                    Err(_) => {
                        operation.operation.status = OperationStatus::Failed;
                        operation.operation.steps[index].status =
                            StepStatus::Failed { tx_hash: None };
                    }
                }

                self.inner.store.save_operation(operation.clone()).await?;
                if operation.operation.status == OperationStatus::Failed {
                    break;
                }
            }

            if operation
                .operation
                .steps
                .iter()
                .all(|step| matches!(step.status, StepStatus::Succeeded { .. }))
            {
                operation.operation.status = OperationStatus::Succeeded;
                self.inner.store.save_operation(operation).await?;
            }
        }
        Ok(())
    }

    fn read_context(&self) -> GatewayContext {
        self.inner.context.clone()
    }
}

fn operation_result_from_stored(operation: StoredOperation) -> WriteOperationResult {
    WriteOperationResult {
        operation: operation.operation,
    }
}

fn request_fingerprint<T: Serialize>(method: &str, params: &T) -> GatewayResult<[u8; 32]> {
    let payload = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "method": method,
        "params": params,
    }))?;
    Ok(Sha256::digest(payload).into())
}
