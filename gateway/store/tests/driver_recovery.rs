//! Recovery/reconciliation behaviour of `OperationDriver` (gateway-core) driven
//! against a real `MemoryStore`, with fake signing/execution so the on-chain
//! results are scripted. Covers reservation reaping (only at startup), ambiguous
//! submitted steps the chain does not know about, per-operation reconciliation
//! serialization, and continuing a multi-step plan after reconciling a submitted
//! step.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use near_api::types::transaction::{
    actions::{Action, TransferAction},
    SignedTransaction, Transaction, TransactionV0,
};
use near_api::types::CryptoHash as NearCryptoHash;
use rstest::rstest;
use templar_gateway_core::{
    CreateOperationResult, CurrentStep, ExecuteOperation, GatewayError, GatewayResult,
    OperationDriver, OperationPlan, OperationStore, PlannedTransaction, PreparedTransactionResult,
    SignTransaction, StepOutcome, StoredOperation, SucceededStep,
};
use templar_gateway_store::MemoryStore;
use templar_gateway_types::{
    operation::{ExecutionOutcome, OperationStatus},
    CryptoHash, IdempotencyKey, ManagedAccountId, NearGas, NearToken, OperationId,
};
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};

fn signer_id() -> ManagedAccountId {
    ManagedAccountId("signer.near".parse().unwrap())
}

fn sample_transaction() -> PlannedTransaction {
    PlannedTransaction::single_action(
        signer_id(),
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

fn submitted_step_at(submitted_at: DateTime<Utc>) -> CurrentStep {
    CurrentStep::Submitted {
        transaction: sample_transaction(),
        tx_hash: CryptoHash(NearCryptoHash::default()),
        submitted_at,
    }
}

/// A step submitted just now — an unknown transaction here remains ambiguous.
fn fresh_submitted_step() -> CurrentStep {
    submitted_step_at(Utc::now())
}

/// A step submitted long ago. `UNKNOWN_TRANSACTION` is still not proof that it
/// never landed, so this remains recoverable rather than rejected.
fn aged_submitted_step() -> CurrentStep {
    submitted_step_at(Utc::now() - TimeDelta::seconds(600))
}

/// A planned operation with a single pending step and no remaining or succeeded steps.
fn step_op(step: CurrentStep) -> StoredOperation {
    stored(true, Some(step), vec![], vec![])
}

/// An unknown transaction may still have landed, so its step is left submitted.
fn is_submitted(step: Option<&CurrentStep>) -> bool {
    matches!(step, Some(CurrentStep::Submitted { .. }))
}

// A well-formed (not cryptographically valid) signed transaction for the fake
// signer; only stored and handed to the fake executor, never broadcast.
fn dummy_signed_transaction() -> SignedTransaction {
    let transaction = Transaction::V0(TransactionV0 {
        signer_id: "signer.near".parse().unwrap(),
        public_key: "ed25519:11111111111111111111111111111111".parse().unwrap(),
        nonce: 1,
        receiver_id: "receiver.near".parse().unwrap(),
        block_hash: NearCryptoHash::default(),
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

#[derive(Default)]
struct QueryProbe {
    active: AtomicUsize,
    max_active: AtomicUsize,
    queries: AtomicUsize,
    entered: Notify,
}

impl QueryProbe {
    async fn wait_for_query(&self) {
        timeout(Duration::from_secs(1), self.entered.notified())
            .await
            .expect("query did not start");
    }

    fn enter_query(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.queries.fetch_add(1, Ordering::SeqCst);
        self.max_active.fetch_max(active, Ordering::SeqCst);
        self.entered.notify_waiters();
    }

    fn exit_query(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ProbeExecutor {
    probe: Arc<QueryProbe>,
    query_delay: Duration,
}

impl ProbeExecutor {
    fn new(probe: Arc<QueryProbe>) -> Self {
        Self {
            probe,
            query_delay: Duration::from_millis(100),
        }
    }
}

#[async_trait]
impl ExecuteOperation for ProbeExecutor {
    async fn submit_transaction(
        &self,
        _signed: SignedTransaction,
    ) -> GatewayResult<Option<StepOutcome>> {
        panic!("unexpected submit_transaction")
    }

    async fn query_transaction(
        &self,
        _signer: &ManagedAccountId,
        _tx_hash: CryptoHash,
    ) -> GatewayResult<StepOutcome> {
        self.probe.enter_query();
        sleep(self.query_delay).await;
        self.probe.exit_query();
        Ok(step_outcome(true))
    }
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

fn driver_with_executor(
    store: Arc<MemoryStore>,
    executor: Arc<dyn ExecuteOperation>,
) -> OperationDriver {
    OperationDriver::new(store, Arc::new(FakeSigner), executor)
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

// ---- reservation reaping only at startup ----

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
        .reconcile_operation(&key)
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
        .reconcile_operation(&IdempotencyKey("missing".to_owned()))
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
        .reconcile_operation(&key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
}

// ---- submitted-step reconciliation ----

#[rstest]
#[case::success(true, OperationStatus::Succeeded)]
#[case::revert(false, OperationStatus::Failed)]
#[tokio::test]
async fn reconcile_resolves_submitted_step(
    #[case] is_success: bool,
    #[case] expected_status: OperationStatus,
) {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(&store, "sub", step_op(fresh_submitted_step())).await;
    let executor = FakeExecutor::new(vec![], vec![Ok(step_outcome(is_success))]);

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, expected_status);
    // Reverted carries an outcome (gas was burnt on chain).
    if !is_success {
        let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
        assert!(matches!(
            op.current_step,
            Some(CurrentStep::Reverted { .. })
        ));
    }
}

/// Reconciling an unknown (chain-has-no-record) submitted transaction leaves it
/// submitted regardless of age: NEAR `UNKNOWN_TRANSACTION` is not proof that it
/// never landed, so the ambiguity must remain recoverable.
#[rstest]
#[case::fresh(fresh_submitted_step(), OperationStatus::InProgress, is_submitted)]
#[case::aged(aged_submitted_step(), OperationStatus::InProgress, is_submitted)]
#[tokio::test]
async fn reconcile_keeps_unknown_submitted_transaction_recoverable(
    #[case] step: CurrentStep,
    #[case] expected_status: OperationStatus,
    #[case] expected_step: fn(Option<&CurrentStep>) -> bool,
) {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(&store, "sub", step_op(step)).await;
    let executor = FakeExecutor::new(vec![], vec![Err(GatewayError::TransactionNotFound)]);

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, expected_status);
    let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
    assert!(expected_step(op.current_step.as_ref()));
}

#[tokio::test]
async fn transient_query_error_leaves_step_submitted() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(&store, "sub", step_op(aged_submitted_step())).await;
    // Even aged, a transient (non-"not found") error must not reject: the tx may
    // have landed.
    let executor = FakeExecutor::new(
        vec![],
        vec![Err(GatewayError::NearTransaction("timeout".to_owned()))],
    );

    let record = driver(store.clone(), executor)
        .reconcile_operation(&key)
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

/// Startup recovery is self-sufficient, but an unknown submitted step remains
/// recoverable regardless of age because `UNKNOWN_TRANSACTION` is ambiguous.
#[rstest]
#[case::fresh(fresh_submitted_step(), is_submitted)]
#[case::aged(aged_submitted_step(), is_submitted)]
#[tokio::test]
async fn startup_recovery_leaves_unknown_submitted_step_recoverable(
    #[case] step: CurrentStep,
    #[case] expected_step: fn(Option<&CurrentStep>) -> bool,
) {
    let store = Arc::new(MemoryStore::new());
    let mut op = step_op(step);
    op.id = OperationId("op".to_owned());
    store.save_operation(op.clone()).await.unwrap();
    let executor = FakeExecutor::new(vec![], vec![Err(GatewayError::TransactionNotFound)]);

    driver(store.clone(), executor)
        .resume_incomplete_operations()
        .await
        .unwrap();

    let op = store.get_by_id(&op.id).await.unwrap().unwrap();
    assert!(expected_step(op.current_step.as_ref()));
}

#[tokio::test]
async fn concurrent_reconcile_for_same_operation_serializes_and_reloads() {
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(&store, "same-op", step_op(fresh_submitted_step())).await;
    let probe = Arc::new(QueryProbe::default());
    let driver = driver_with_executor(store, Arc::new(ProbeExecutor::new(probe.clone())));

    // Given: one reconcile has entered the chain query for the operation.
    let first_query = probe.entered.notified();
    let first_driver = driver.clone();
    let first_key = key.clone();
    let first = tokio::spawn(async move { first_driver.reconcile_operation(&first_key).await });
    first_query.await;

    // When: a second reconcile for the same idempotency key races it.
    let second_driver = driver.clone();
    let second_key = key.clone();
    let second = tokio::spawn(async move { second_driver.reconcile_operation(&second_key).await });

    // Then: the second caller waits, reloads the terminal operation, and does not
    // issue a duplicate chain query from stale Submitted state.
    first.await.unwrap().unwrap().unwrap();
    second.await.unwrap().unwrap().unwrap();
    assert_eq!(probe.queries.load(Ordering::SeqCst), 1);
    assert_eq!(probe.max_active.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_reconcile_for_different_operations_is_not_globally_serialized() {
    let store = Arc::new(MemoryStore::new());
    let first_key = seed_keyed(&store, "first-op", step_op(fresh_submitted_step())).await;
    let second_key = seed_keyed(&store, "second-op", step_op(fresh_submitted_step())).await;
    let probe = Arc::new(QueryProbe::default());
    let driver = driver_with_executor(store, Arc::new(ProbeExecutor::new(probe.clone())));

    // Given: one operation is already reconciling and waiting on the chain query.
    let first_query = probe.entered.notified();
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.reconcile_operation(&first_key).await });
    first_query.await;

    // When: a different operation reconciles concurrently.
    let second_driver = driver.clone();
    let second = tokio::spawn(async move { second_driver.reconcile_operation(&second_key).await });
    probe.wait_for_query().await;

    // Then: both operations can be in their chain queries at once; the driver did
    // not use a process-wide advancement lock.
    first.await.unwrap().unwrap().unwrap();
    second.await.unwrap().unwrap().unwrap();
    assert_eq!(probe.queries.load(Ordering::SeqCst), 2);
    assert_eq!(probe.max_active.load(Ordering::SeqCst), 2);
}

// ---- multi-step operations are driven to completion ----

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
    // Reconciling a submitted step must not leave the rest of a multi-step plan
    // un-driven.
    let store = Arc::new(MemoryStore::new());
    let key = seed_keyed(
        &store,
        "multi",
        stored(
            true,
            Some(fresh_submitted_step()),
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
        .reconcile_operation(&key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
    let op = store.get_by_idempotency_key(&key).await.unwrap().unwrap();
    assert_eq!(op.succeeded_steps.len(), 2);
}
