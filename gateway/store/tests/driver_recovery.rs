//! Recovery/reconciliation behaviour of `OperationDriver` (gateway-core) driven
//! against a real `MemoryStore`, with fake signing/execution so the on-chain
//! results are scripted. Covers item 1 (reservations are reaped only at startup)
//! and item 2 (submitted-step reconciliation continues a multi-step plan).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use near_api::types::transaction::{
    actions::{Action, TransferAction},
    SignedTransaction, Transaction, TransactionV0,
};
use near_api::types::CryptoHash as NearCryptoHash;
use templar_gateway_core::{
    CreateOperationResult, CurrentStep, ExecuteOperation, GatewayError, GatewayResult,
    OperationDriver, OperationPlan, OperationStore, PlannedTransaction, PreparedTransactionResult,
    SignTransaction, StepOutcome, StoredOperation, SucceededStep,
};
use templar_gateway_store::MemoryStore;
use templar_gateway_types::{
    common::TxExecutionStatus,
    operation::{ExecutionOutcome, OperationStatus},
    CryptoHash, IdempotencyKey, ManagedAccountId, NearGas, NearToken, OperationId,
};

fn signer_id() -> ManagedAccountId {
    ManagedAccountId("signer.near".parse().unwrap())
}

fn sample_transaction() -> PlannedTransaction {
    PlannedTransaction::single_action(
        signer_id(),
        TxExecutionStatus::Final,
        "receiver.near".parse().unwrap(),
        Action::Transfer(TransferAction {
            deposit: NearToken::from_yoctonear(1),
        }),
    )
}

fn sample_outcome() -> ExecutionOutcome {
    ExecutionOutcome {
        tokens_burnt: NearToken::from_yoctonear(1),
        total_gas_burnt: NearGas::from_gas(1),
        receipts: vec![],
        return_value: None,
    }
}

fn step_outcome(is_success: bool) -> StepOutcome {
    StepOutcome {
        tx_hash: CryptoHash(NearCryptoHash::default()),
        is_success,
        outcome: sample_outcome(),
    }
}

fn succeeded_step() -> SucceededStep {
    SucceededStep {
        transaction: sample_transaction(),
        tx_hash: CryptoHash(NearCryptoHash::default()),
        outcome: sample_outcome(),
    }
}

fn submitted_step() -> CurrentStep {
    CurrentStep::Submitted {
        transaction: sample_transaction(),
        tx_hash: CryptoHash(NearCryptoHash::default()),
    }
}

// A well-formed (not cryptographically valid) signed transaction for the fake
// signer; only stored and handed to the fake executor, never broadcast.
fn dummy_signed_transaction() -> SignedTransaction {
    let transaction = Transaction::V0(TransactionV0 {
        signer_id: "signer.near".parse().unwrap(),
        public_key: "ed25519:11111111111111111111111111111111".parse().unwrap(),
        nonce: 1,
        receiver_id: "receiver.near".parse().unwrap(),
        block_hash: Default::default(),
        actions: vec![],
    });
    let signature = "ed25519:1111111111111111111111111111111111111111111111111111111111111111"
        .parse()
        .unwrap();
    SignedTransaction::new(signature, transaction)
}

struct FakeSigner;

#[async_trait]
impl SignTransaction for FakeSigner {
    async fn sign_transaction(
        &self,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        Ok(PreparedTransactionResult {
            transaction,
            tx_hash: CryptoHash(NearCryptoHash::default()),
            signed_transaction: dummy_signed_transaction(),
        })
    }
}

/// Executor that replays canned results in order; panics on an unexpected call,
/// so each test asserts exactly which chain interactions happen.
#[derive(Default)]
struct FakeExecutor {
    submits: Mutex<VecDeque<GatewayResult<Option<StepOutcome>>>>,
    queries: Mutex<VecDeque<GatewayResult<StepOutcome>>>,
}

impl FakeExecutor {
    fn new(
        submits: Vec<GatewayResult<Option<StepOutcome>>>,
        queries: Vec<GatewayResult<StepOutcome>>,
    ) -> Self {
        Self {
            submits: Mutex::new(submits.into()),
            queries: Mutex::new(queries.into()),
        }
    }
}

#[async_trait]
impl ExecuteOperation for FakeExecutor {
    async fn submit_transaction(
        &self,
        _signed: SignedTransaction,
        _wait_until: TxExecutionStatus,
    ) -> GatewayResult<Option<StepOutcome>> {
        self.submits
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected submit_transaction")
    }

    async fn query_transaction(
        &self,
        _signer: &ManagedAccountId,
        _tx_hash: CryptoHash,
    ) -> GatewayResult<StepOutcome> {
        self.queries
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected query_transaction")
    }
}

fn driver(store: Arc<MemoryStore>, executor: FakeExecutor) -> OperationDriver {
    OperationDriver::new(store, Arc::new(FakeSigner), Arc::new(executor))
}

fn stored(
    planned: bool,
    current_step: Option<CurrentStep>,
    remaining: Vec<PlannedTransaction>,
    succeeded: Vec<SucceededStep>,
) -> StoredOperation {
    StoredOperation {
        rpc_method: "tx.transfer".to_owned(),
        request_fingerprint_hash: [0; 32],
        request_payload: vec![],
        id: OperationId("op-under-test".to_owned()),
        signer_account_id: signer_id(),
        planned,
        succeeded_steps: succeeded,
        current_step,
        remaining_steps: remaining.into(),
    }
}

async fn reservation_with_key(store: &Arc<MemoryStore>, key: &str) -> IdempotencyKey {
    let key = IdempotencyKey(key.to_owned());
    store
        .create_or_get_operation(
            "tx.transfer",
            signer_id(),
            Some(key.clone()),
            [7; 32],
            vec![],
            OperationPlan { steps: vec![] },
        )
        .await
        .unwrap();
    key
}

/// Register an idempotency key (as a reservation) then overwrite it with the
/// desired state, reusing the id so the key still resolves.
async fn seed_keyed(
    store: &Arc<MemoryStore>,
    key: &str,
    mut op: StoredOperation,
) -> IdempotencyKey {
    let key = IdempotencyKey(key.to_owned());
    let created = store
        .create_or_get_operation(
            "tx.transfer",
            signer_id(),
            Some(key.clone()),
            [7; 32],
            vec![],
            OperationPlan { steps: vec![] },
        )
        .await
        .unwrap();
    let CreateOperationResult::Created(reservation) = created else {
        panic!("expected a fresh reservation");
    };
    op.id = reservation.id;
    store.save_operation(op).await.unwrap();
    key
}

// ---- item 1: reservation reaping only at startup ----

#[tokio::test]
async fn startup_recovery_reaps_reservations() {
    let store = Arc::new(MemoryStore::new());
    let created = store
        .create_or_get_operation(
            "tx.transfer",
            signer_id(),
            None,
            [1; 32],
            vec![],
            OperationPlan { steps: vec![] },
        )
        .await
        .unwrap();
    let CreateOperationResult::Created(reservation) = created else {
        panic!("expected a reservation");
    };
    assert!(reservation.is_reservation());

    driver(store.clone(), FakeExecutor::default())
        .resume_incomplete_operations()
        .await
        .unwrap();

    assert!(
        store.get_by_id(&reservation.id).await.unwrap().is_none(),
        "startup recovery reaps a dead reservation"
    );
}

#[tokio::test]
async fn reconcile_operation_leaves_reservations_intact() {
    let store = Arc::new(MemoryStore::new());
    let key = reservation_with_key(&store, "live").await;

    // The broom must never reap a reservation: its request may still be planning.
    let record = driver(store.clone(), FakeExecutor::default())
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .expect("reservation is returned, not dropped");
    assert_eq!(record.status, OperationStatus::Pending);
    assert!(store.get_by_idempotency_key(&key).await.unwrap().is_some());
}

#[tokio::test]
async fn reconcile_operation_unknown_key_returns_none() {
    let store = Arc::new(MemoryStore::new());
    let record = driver(store, FakeExecutor::default())
        .reconcile_operation(&IdempotencyKey("missing".to_owned()), true)
        .await
        .unwrap();
    assert!(record.is_none());
}

#[tokio::test]
async fn reconcile_operation_terminal_passes_through_without_chain_calls() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "done",
        stored(true, None, vec![], vec![succeeded_step()]),
    )
    .await;

    // FakeExecutor::default() panics if any chain call is made.
    let record = driver(store, FakeExecutor::default())
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
}

// ---- submitted-step reconciliation ----

#[tokio::test]
async fn reconcile_operation_resolves_submitted_step_success() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "sub",
        stored(true, Some(submitted_step()), vec![], vec![]),
    )
    .await;
    let executor = FakeExecutor::new(vec![], vec![Ok(step_outcome(true))]);

    let record = driver(store, executor)
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
}

#[tokio::test]
async fn reconcile_operation_resolves_submitted_step_revert() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "sub",
        stored(true, Some(submitted_step()), vec![], vec![]),
    )
    .await;
    let executor = FakeExecutor::new(vec![], vec![Ok(step_outcome(false))]);

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Failed);
    // Reverted carries an outcome (gas was burnt on chain).
    let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
    assert!(matches!(
        op.current_step,
        Some(CurrentStep::Reverted { .. })
    ));
}

#[tokio::test]
async fn broom_rejects_aged_out_unknown_transaction() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "sub",
        stored(true, Some(submitted_step()), vec![], vec![]),
    )
    .await;
    let executor = FakeExecutor::new(vec![], vec![Err(GatewayError::TransactionNotFound)]);

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Failed);
    // Rejected: never landed, so no outcome (its charge is released).
    let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
    assert!(matches!(
        op.current_step,
        Some(CurrentStep::Rejected { .. })
    ));
}

#[tokio::test]
async fn transient_query_error_leaves_step_submitted() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "sub",
        stored(true, Some(submitted_step()), vec![], vec![]),
    )
    .await;
    let executor = FakeExecutor::new(
        vec![],
        vec![Err(GatewayError::NearTransaction("timeout".to_owned()))],
    );

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::InProgress);
    let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
    assert!(matches!(
        op.current_step,
        Some(CurrentStep::Submitted { .. })
    ));
}

#[tokio::test]
async fn startup_recovery_never_rejects_a_fresh_unknown() {
    // reject_if_unknown = false during recovery: a still-unknown tx is left
    // Submitted for the broom to age out, not rejected.
    let store = Arc::new(MemoryStore::new());
    let mut op = stored(true, Some(submitted_step()), vec![], vec![]);
    op.id = OperationId("fresh".to_owned());
    store.save_operation(op.clone()).await.unwrap();
    let executor = FakeExecutor::new(vec![], vec![Err(GatewayError::TransactionNotFound)]);

    driver(store.clone(), executor)
        .resume_incomplete_operations()
        .await
        .unwrap();

    let op = store.get_by_id(&op.id).await.unwrap().unwrap();
    assert!(matches!(
        op.current_step,
        Some(CurrentStep::Submitted { .. })
    ));
}

// ---- item 2: multi-step operations are driven to completion ----

#[tokio::test]
async fn recovery_drives_a_multi_step_plan_to_completion() {
    let store = Arc::new(MemoryStore::new());
    let mut op = stored(
        true,
        None,
        vec![sample_transaction(), sample_transaction()],
        vec![],
    );
    op.id = OperationId("multi".to_owned());
    store.save_operation(op.clone()).await.unwrap();
    let executor = FakeExecutor::new(
        vec![Ok(Some(step_outcome(true))), Ok(Some(step_outcome(true)))],
        vec![],
    );

    driver(store.clone(), executor)
        .resume_incomplete_operations()
        .await
        .unwrap();

    let op = store.get_by_id(&op.id).await.unwrap().unwrap();
    assert_eq!(op.status(), OperationStatus::Succeeded);
    assert_eq!(op.succeeded_steps.len(), 2);
}

#[tokio::test]
async fn reconcile_continues_remaining_steps_after_a_submitted_step_lands() {
    // The item-2 regression: reconciling a submitted step must not leave the rest
    // of a multi-step plan un-driven.
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "multi",
        stored(
            true,
            Some(submitted_step()),
            vec![sample_transaction()],
            vec![],
        ),
    )
    .await;
    // query: step 1 landed; submit: step 2 is driven.
    let executor = FakeExecutor::new(
        vec![Ok(Some(step_outcome(true)))],
        vec![Ok(step_outcome(true))],
    );

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
    let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
    assert_eq!(op.succeeded_steps.len(), 2);
}
