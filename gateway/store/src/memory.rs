use std::collections::{HashMap, VecDeque};

use async_trait::async_trait;
use templar_gateway_core::{
    CreateOperationResult, GatewayError, GatewayResult, OperationPlan, OperationStore,
    StoredOperation,
};
use templar_gateway_types::{operation::OperationId, IdempotencyKey, ManagedAccountId};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
pub struct MemoryStore {
    state: Mutex<MemoryStoreState>,
}

#[derive(Default)]
struct MemoryStoreState {
    operations: HashMap<OperationId, StoredOperation>,
    idempotency: HashMap<IdempotencyKey, OperationId>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OperationStore for MemoryStore {
    async fn get_by_id(
        &self,
        operation_id: &OperationId,
    ) -> GatewayResult<Option<StoredOperation>> {
        Ok(self
            .state
            .lock()
            .await
            .operations
            .get(operation_id)
            .cloned())
    }

    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>> {
        let state = self.state.lock().await;
        let operation_id = state.idempotency.get(idempotency_key).cloned();
        let Some(operation_id) = operation_id else {
            return Ok(None);
        };

        Ok(state.operations.get(&operation_id).cloned())
    }

    async fn create_or_get_operation(
        &self,
        rpc_method: &str,
        signer_account_id: ManagedAccountId,
        idempotency_key: Option<IdempotencyKey>,
        request_fingerprint_hash: [u8; 32],
        request_payload: Vec<u8>,
        plan: OperationPlan,
    ) -> GatewayResult<CreateOperationResult> {
        let mut state = self.state.lock().await;

        if let Some(idempotency_key) = &idempotency_key {
            if let Some(operation_id) = state.idempotency.get(idempotency_key) {
                let existing = state
                    .operations
                    .get(operation_id)
                    .cloned()
                    .expect("idempotency mapping should reference existing operation");
                if existing.request_fingerprint_hash != request_fingerprint_hash {
                    return Err(GatewayError::IdempotencyConflict);
                }
                return Ok(CreateOperationResult::Existing(existing));
            }
        }

        let operation = StoredOperation {
            rpc_method: rpc_method.to_owned(),
            request_fingerprint_hash,
            request_payload,
            id: OperationId(Uuid::new_v4().to_string()),
            signer_account_id,
            succeeded_steps: vec![],
            current_step: None,
            remaining_steps: VecDeque::from(plan.steps),
        };

        if let Some(idempotency_key) = idempotency_key {
            state
                .idempotency
                .insert(idempotency_key, operation.operation_id().clone());
        }
        state
            .operations
            .insert(operation.operation_id().clone(), operation.clone());
        Ok(CreateOperationResult::Created(operation))
    }

    async fn save_operation(&self, operation: StoredOperation) -> GatewayResult<()> {
        self.state
            .lock()
            .await
            .operations
            .insert(operation.operation_id().clone(), operation);
        Ok(())
    }

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>> {
        Ok(self
            .state
            .lock()
            .await
            .operations
            .values()
            .filter(|operation| {
                matches!(
                    operation.status(),
                    templar_gateway_types::OperationStatus::Pending
                        | templar_gateway_types::OperationStatus::InProgress
                )
            })
            .cloned()
            .collect())
    }
}
