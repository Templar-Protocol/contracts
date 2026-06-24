use std::collections::{HashMap, VecDeque};

use async_trait::async_trait;
use indexmap::IndexSet;
use templar_gateway_core::{
    CreateOperationResult, GatewayError, GatewayResult, OperationPlan, OperationStore,
    StoredOperation,
};
use templar_gateway_types::{
    operation::OperationId, IdempotencyKey, ManagedAccountId, OperationStatus,
};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Default cap on retained completed (terminal) operations.
///
/// In-flight operations are bounded by concurrency and never evicted; this only
/// limits how many finished operations we keep around for poll-by-id and
/// idempotency dedup. See [`MemoryStore::with_capacity`].
pub const DEFAULT_MAX_COMPLETED_OPERATIONS: usize = 4096;

/// In-memory [`OperationStore`] for ephemeral/in-process use.
///
/// Completed operations are retained up to a bounded window
/// ([`MemoryStore::with_capacity`], default
/// [`DEFAULT_MAX_COMPLETED_OPERATIONS`]) so a long-running consumer that streams
/// un-keyed operations does not grow without bound. Consumers needing durable
/// idempotency/replay beyond this window use `PostgresStore`.
pub struct MemoryStore {
    state: Mutex<MemoryStoreState>,
    max_completed_operations: usize,
}

#[derive(Default)]
struct MemoryStoreState {
    operations: HashMap<OperationId, StoredOperation>,
    idempotency: HashMap<IdempotencyKey, OperationId>,
    /// Reverse index so an evicted operation can drop its idempotency mapping.
    idempotency_by_id: HashMap<OperationId, IdempotencyKey>,
    /// Terminal operations in completion order (oldest first). An ordered set
    /// gives FIFO eviction *and* O(1) dedupe of repeated terminal saves in a
    /// single field, so the order/membership invariant can't drift.
    completed: IndexSet<OperationId>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a store retaining at most `max_completed_operations` completed
    /// operations (clamped to at least 1).
    #[must_use]
    pub fn with_capacity(max_completed_operations: usize) -> Self {
        Self {
            state: Mutex::new(MemoryStoreState::default()),
            max_completed_operations: max_completed_operations.max(1),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_MAX_COMPLETED_OPERATIONS)
    }
}

/// Whether an operation has reached a terminal (`Succeeded`/`Failed`) status.
fn is_terminal(operation: &StoredOperation) -> bool {
    matches!(
        operation.status(),
        OperationStatus::Succeeded | OperationStatus::Failed
    )
}

impl MemoryStoreState {
    /// Record a terminal operation and evict the oldest completed operations
    /// beyond the retention cap, dropping their entries from every index.
    fn record_completion_and_evict(&mut self, operation_id: OperationId, max_completed: usize) {
        // `insert` keeps insertion order and dedupes a repeated terminal save.
        self.completed.insert(operation_id);

        while self.completed.len() > max_completed {
            let Some(evicted) = self.completed.shift_remove_index(0) else {
                break;
            };
            self.operations.remove(&evicted);
            if let Some(key) = self.idempotency_by_id.remove(&evicted) {
                self.idempotency.remove(&key);
            }
        }
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
                let existing = state.operations.get(operation_id).cloned().ok_or_else(|| {
                    GatewayError::InvalidStoredOperation(
                        "idempotency mapping references missing operation".to_owned(),
                    )
                })?;
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
                .insert(idempotency_key.clone(), operation.operation_id().clone());
            state
                .idempotency_by_id
                .insert(operation.operation_id().clone(), idempotency_key);
        }
        state
            .operations
            .insert(operation.operation_id().clone(), operation.clone());
        Ok(CreateOperationResult::Created(operation))
    }

    async fn save_operation(&self, operation: StoredOperation) -> GatewayResult<()> {
        let mut state = self.state.lock().await;
        let operation_id = operation.operation_id().clone();
        let terminal = is_terminal(&operation);
        state.operations.insert(operation_id.clone(), operation);
        if terminal {
            state.record_completion_and_evict(operation_id, self.max_completed_operations);
        }
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
                    OperationStatus::Pending | OperationStatus::InProgress
                )
            })
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_account_id::AccountId;
    use templar_gateway_core::PlannedTransaction;
    use templar_gateway_types::common::TxExecutionStatus;

    fn signer() -> ManagedAccountId {
        "signer.near".parse::<AccountId>().unwrap().into()
    }

    /// A finished (terminal `Succeeded`) operation: no current step, no remaining
    /// steps.
    fn terminal_operation(id: &str) -> StoredOperation {
        StoredOperation {
            rpc_method: "market.applyInterest".to_owned(),
            request_fingerprint_hash: [0_u8; 32],
            request_payload: Vec::new(),
            id: OperationId(id.to_owned()),
            signer_account_id: signer(),
            succeeded_steps: Vec::new(),
            current_step: None,
            remaining_steps: VecDeque::new(),
        }
    }

    /// An in-flight (`Pending`) operation: one remaining step, nothing run yet.
    fn pending_operation(id: &str) -> StoredOperation {
        let mut operation = terminal_operation(id);
        operation.remaining_steps.push_back(PlannedTransaction {
            signer_account_id: signer(),
            wait_until: TxExecutionStatus::Final,
            receiver_id: "market.near".parse().unwrap(),
            actions: Vec::new(),
        });
        operation
    }

    #[tokio::test]
    async fn evicts_oldest_completed_beyond_capacity() {
        let store = MemoryStore::with_capacity(3);
        let ids: Vec<String> = (0..5).map(|i| format!("op-{i}")).collect();

        for id in &ids {
            store.save_operation(terminal_operation(id)).await.unwrap();
        }

        // Only the 3 most recent terminal operations are retained.
        assert!(store
            .get_by_id(&OperationId(ids[0].clone()))
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get_by_id(&OperationId(ids[1].clone()))
            .await
            .unwrap()
            .is_none());
        for id in &ids[2..] {
            assert!(
                store
                    .get_by_id(&OperationId(id.clone()))
                    .await
                    .unwrap()
                    .is_some(),
                "expected {id} to be retained"
            );
        }

        let state = store.state.lock().await;
        assert_eq!(state.operations.len(), 3);
        assert_eq!(state.completed.len(), 3);
    }

    #[tokio::test]
    async fn repeated_terminal_save_does_not_double_count() {
        let store = MemoryStore::with_capacity(2);

        // Save the same terminal operation several times (mirrors resume/reconcile
        // re-saving an already-finished operation).
        for _ in 0..5 {
            store
                .save_operation(terminal_operation("op"))
                .await
                .unwrap();
        }

        let state = store.state.lock().await;
        assert_eq!(state.completed.len(), 1);
        assert_eq!(state.operations.len(), 1);
    }

    #[tokio::test]
    async fn does_not_evict_in_flight_operations() {
        let store = MemoryStore::with_capacity(1);

        // Many in-flight operations: none are terminal, so none are evicted even
        // though they far exceed the completed-operation cap.
        for i in 0..10 {
            store
                .save_operation(pending_operation(&format!("pending-{i}")))
                .await
                .unwrap();
        }

        let state = store.state.lock().await;
        assert_eq!(state.operations.len(), 10);
        assert!(state.completed.is_empty());
    }

    #[tokio::test]
    async fn eviction_drops_idempotency_mapping() {
        let store = MemoryStore::with_capacity(1);
        let key_a = IdempotencyKey("key-a".to_owned());
        let key_b = IdempotencyKey("key-b".to_owned());

        let plan = OperationPlan { steps: Vec::new() };
        let make = |key: &IdempotencyKey| {
            store.create_or_get_operation(
                "market.applyInterest",
                signer(),
                Some(key.clone()),
                [0_u8; 32],
                Vec::new(),
                OperationPlan {
                    steps: plan.steps.clone(),
                },
            )
        };

        let CreateOperationResult::Created(op_a) = make(&key_a).await.unwrap() else {
            panic!("expected a freshly created operation");
        };
        store.save_operation(op_a.clone()).await.unwrap();

        let CreateOperationResult::Created(op_b) = make(&key_b).await.unwrap() else {
            panic!("expected a freshly created operation");
        };
        store.save_operation(op_b.clone()).await.unwrap();

        // op_a was evicted: gone by id and its idempotency mapping is cleared.
        assert!(store
            .get_by_id(op_a.operation_id())
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get_by_idempotency_key(&key_a)
            .await
            .unwrap()
            .is_none());

        // op_b (the survivor) is still reachable both ways.
        assert!(store
            .get_by_id(op_b.operation_id())
            .await
            .unwrap()
            .is_some());
        assert!(store
            .get_by_idempotency_key(&key_b)
            .await
            .unwrap()
            .is_some());

        let state = store.state.lock().await;
        assert_eq!(state.idempotency.len(), 1);
        assert_eq!(state.idempotency_by_id.len(), 1);
    }

    #[tokio::test]
    async fn keyed_op_evicted_by_unkeyed_completions_cleans_idempotency() {
        let store = MemoryStore::with_capacity(2);
        let key = IdempotencyKey("keep-me".to_owned());

        // A single keyed terminal operation.
        let CreateOperationResult::Created(keyed) = store
            .create_or_get_operation(
                "market.applyInterest",
                signer(),
                Some(key.clone()),
                [0_u8; 32],
                Vec::new(),
                OperationPlan { steps: Vec::new() },
            )
            .await
            .unwrap()
        else {
            panic!("expected a freshly created operation");
        };
        store.save_operation(keyed.clone()).await.unwrap();

        // Flood the window with un-keyed terminal completions, pushing the keyed
        // op out of the retention window.
        for i in 0..5 {
            store
                .save_operation(terminal_operation(&format!("unkeyed-{i}")))
                .await
                .unwrap();
        }

        // The keyed op is gone from every index — including both idempotency maps.
        assert!(store
            .get_by_id(keyed.operation_id())
            .await
            .unwrap()
            .is_none());
        assert!(store.get_by_idempotency_key(&key).await.unwrap().is_none());

        let state = store.state.lock().await;
        assert_eq!(state.operations.len(), 2);
        assert!(state.idempotency.is_empty());
        assert!(state.idempotency_by_id.is_empty());
    }
}
