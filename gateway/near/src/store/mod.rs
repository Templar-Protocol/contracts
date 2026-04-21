use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use blockchain_gateway_core::{operation::OperationId, IdempotencyKey, ManagedAccountId};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    operation::{OperationPlan, StoredOperation},
    GatewayResult,
};
use std::collections::VecDeque;

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
            id: OperationId(Uuid::new_v4().to_string()),
            signer_account_id,
            succeeded_steps: vec![],
            current_step: None,
            remaining_steps: VecDeque::from(plan.steps),
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
                    operation.status(),
                    blockchain_gateway_core::OperationStatus::Pending
                        | blockchain_gateway_core::OperationStatus::InProgress
                )
            })
            .cloned()
            .collect())
    }
}

pub type SharedOperationStore = Arc<dyn OperationStore>;
