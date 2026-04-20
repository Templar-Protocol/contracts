use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use blockchain_gateway_core::{
    operation::{OperationId, OperationRecord, OperationStatus, StepStatus, TransactionStepRecord},
    IdempotencyKey, ManagedAccountId,
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    operation::{OperationPlan, StoredOperation},
    GatewayResult,
};

pub mod postgres;

#[derive(Default)]
pub struct MemoryOperationStore {
    operations: Mutex<HashMap<OperationId, StoredOperation>>,
    idempotency: Mutex<HashMap<IdempotencyKey, OperationId>>,
}

#[async_trait]
pub trait OperationStore: Send + Sync {
    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>>;

    async fn create_operation(
        &self,
        signer_account_id: ManagedAccountId,
        idempotency_key: Option<IdempotencyKey>,
        request_fingerprint_hash: [u8; 32],
        request_payload: Vec<u8>,
        plan: OperationPlan,
    ) -> GatewayResult<StoredOperation>;

    async fn save_operation(&self, operation: StoredOperation) -> GatewayResult<()>;

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>>;
}

impl MemoryOperationStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OperationStore for MemoryOperationStore {
    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>> {
        let operation_id = self.idempotency.lock().await.get(idempotency_key).cloned();
        let Some(operation_id) = operation_id else {
            return Ok(None);
        };

        Ok(self.operations.lock().await.get(&operation_id).cloned())
    }

    async fn create_operation(
        &self,
        signer_account_id: ManagedAccountId,
        idempotency_key: Option<IdempotencyKey>,
        request_fingerprint_hash: [u8; 32],
        request_payload: Vec<u8>,
        plan: OperationPlan,
    ) -> GatewayResult<StoredOperation> {
        let operation = StoredOperation {
            request_fingerprint_hash,
            request_payload,
            operation: OperationRecord {
                id: OperationId(Uuid::new_v4().to_string()),
                signer_account_id,
                status: OperationStatus::Pending,
                steps: plan
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(index, _)| TransactionStepRecord {
                        index: index as u32,
                        status: StepStatus::NotStarted,
                    })
                    .collect(),
            },
            plan,
        };

        if let Some(idempotency_key) = idempotency_key {
            self.idempotency
                .lock()
                .await
                .insert(idempotency_key, operation.operation_id().clone());
        }
        self.operations
            .lock()
            .await
            .insert(operation.operation_id().clone(), operation.clone());
        Ok(operation)
    }

    async fn save_operation(&self, operation: StoredOperation) -> GatewayResult<()> {
        self.operations
            .lock()
            .await
            .insert(operation.operation_id().clone(), operation);
        Ok(())
    }

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>> {
        Ok(self
            .operations
            .lock()
            .await
            .values()
            .filter(|operation| {
                matches!(
                    operation.operation.status,
                    OperationStatus::Pending | OperationStatus::InProgress
                )
            })
            .cloned()
            .collect())
    }
}

pub type SharedOperationStore = Arc<dyn OperationStore>;
