//! Recovery/reconciliation behaviour of `OperationDriver` (gateway-core) driven
//! against a real `MemoryStore`, with fake signing/execution so the on-chain
//! results are scripted. Covers reservation reaping (only at startup), ambiguous
//! submitted steps the chain does not know about, per-operation reconciliation
//! serialization, and continuing a multi-step plan after reconciling a submitted
//! step.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use near_api::types::crypto::secret_key::ED25519SecretKey;
use near_api::types::transaction::{
    actions::{Action, TransferAction},
    SignedTransaction, Transaction, TransactionV0,
};
use near_api::types::CryptoHash as NearCryptoHash;
use near_api::{PublicKey, SecretKey};
use rstest::rstest;
use templar_gateway_core::{
    CompletedStep, CreateOperationResult, CurrentStep, ExecuteOperation, GatewayError,
    GatewayResult, OperationDriver, OperationPlan, OperationStore, PlannedTransaction,
    PooledSigner, PreparedTransactionResult, SignTransaction, SigningKeyLease, StepOutcome,
    StoredOperation, SucceededStep,
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
        failure: None,
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

fn pool_secret_key(index: u8) -> SecretKey {
    SecretKey::ED25519(ED25519SecretKey::from_secret_key([index + 1; 32]))
}

fn pool_public_key(index: u8) -> PublicKey {
    pool_secret_key(index).public_key()
}

// A well-formed (not cryptographically valid) signed transaction for the fake
// signer; only stored and handed to the fake executor, never broadcast. The key
// and nonce are the observable the concurrency tests assert on.
fn dummy_signed_transaction(public_key: PublicKey, nonce: u64) -> SignedTransaction {
    let transaction = Transaction::V0(TransactionV0 {
        signer_id: "signer.near".parse().unwrap(),
        public_key,
        nonce,
        receiver_id: "receiver.near".parse().unwrap(),
        block_hash: NearCryptoHash::default(),
        actions: vec![],
    });
    let signature = "ed25519:1111111111111111111111111111111111111111111111111111111111111111"
        .parse()
        .unwrap();
    SignedTransaction::new(signature, transaction)
}

/// Leases against a real [`PooledSigner`], so tests exercise the production
/// per-key serialization; only the signed transaction is fabricated. Nonces are
/// allocated per key, mirroring near-api's cache.
struct FakeSigner {
    pool: PooledSigner,
    nonces: Mutex<HashMap<PublicKey, u64>>,
}

impl FakeSigner {
    fn with_keys(count: u8) -> Self {
        Self {
            pool: PooledSigner::new(signer_id(), (0..count).map(pool_secret_key)).unwrap(),
            nonces: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for FakeSigner {
    fn default() -> Self {
        Self::with_keys(1)
    }
}

#[async_trait]
impl SignTransaction for FakeSigner {
    async fn lease_next_signing_key(
        &self,
        _signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<SigningKeyLease> {
        Ok(self.pool.lease_next().await)
    }

    async fn lease_signing_key(
        &self,
        _signer_account_id: &ManagedAccountId,
        public_key: &PublicKey,
    ) -> Option<SigningKeyLease> {
        self.pool.lease(public_key).await
    }

    async fn sign_transaction(
        &self,
        lease: &SigningKeyLease,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        let public_key = lease.public_key();
        let nonce = {
            let mut nonces = self.nonces.lock().unwrap();
            let next = nonces.entry(public_key).or_default();
            *next += 1;
            *next
        };
        Ok(PreparedTransactionResult {
            transaction,
            tx_hash: CryptoHash(NearCryptoHash::default()),
            signed_transaction: dummy_signed_transaction(public_key, nonce),
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

#[derive(Default)]
struct BroadcastState {
    /// `(public_key, nonce)` in the order the executor received them.
    order: Vec<(PublicKey, u64)>,
    active: HashMap<PublicKey, usize>,
    max_active_per_key: usize,
    active_total: usize,
    max_active_total: usize,
}

/// Records what actually reached the network, and how much of it overlapped.
#[derive(Default)]
struct BroadcastLog {
    state: Mutex<BroadcastState>,
}

impl BroadcastLog {
    fn enter(&self, public_key: PublicKey, nonce: u64) {
        let mut state = self.state.lock().unwrap();
        state.order.push((public_key, nonce));

        let per_key = state.active.entry(public_key).or_default();
        *per_key += 1;
        let per_key = *per_key;
        state.max_active_per_key = state.max_active_per_key.max(per_key);

        state.active_total += 1;
        state.max_active_total = state.max_active_total.max(state.active_total);
    }

    fn exit(&self, public_key: PublicKey) {
        let mut state = self.state.lock().unwrap();
        *state.active.entry(public_key).or_default() -= 1;
        state.active_total -= 1;
    }

    fn nonces_for(&self, public_key: PublicKey) -> Vec<u64> {
        self.state
            .lock()
            .unwrap()
            .order
            .iter()
            .filter(|(key, _)| *key == public_key)
            .map(|(_, nonce)| *nonce)
            .collect()
    }

    fn broadcasts(&self) -> usize {
        self.state.lock().unwrap().order.len()
    }

    fn max_active_per_key(&self) -> usize {
        self.state.lock().unwrap().max_active_per_key
    }

    fn max_active_total(&self) -> usize {
        self.state.lock().unwrap().max_active_total
    }
}

struct BroadcastExecutor {
    log: Arc<BroadcastLog>,
    broadcast_delay: Duration,
}

#[async_trait]
impl ExecuteOperation for BroadcastExecutor {
    async fn submit_transaction(
        &self,
        signed: SignedTransaction,
    ) -> GatewayResult<Option<StepOutcome>> {
        let public_key = signed.transaction.public_key();
        self.log.enter(public_key, signed.transaction.nonce());
        sleep(self.broadcast_delay).await;
        self.log.exit(public_key);
        Ok(Some(step_outcome(true)))
    }

    async fn query_transaction(
        &self,
        _signer: &ManagedAccountId,
        _tx_hash: CryptoHash,
    ) -> GatewayResult<StepOutcome> {
        panic!("unexpected query_transaction")
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
    OperationDriver::new(store, Arc::new(FakeSigner::default()), Arc::new(executor))
}

fn driver_with_executor(
    store: Arc<MemoryStore>,
    executor: Arc<dyn ExecuteOperation>,
) -> OperationDriver {
    OperationDriver::new(store, Arc::new(FakeSigner::default()), executor)
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
        completed_steps: succeeded
            .into_iter()
            .map(CompletedStep::Succeeded)
            .collect(),
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
    assert_eq!(op.completed_steps.len(), 2);
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
    assert_eq!(op.completed_steps.len(), 2);
}

// ---- continue_on_failure step tolerance ----

/// A `continue_on_failure` step that reverts on chain must be recorded and
/// tolerated: the operation advances to the next step and can still succeed.
#[tokio::test]
async fn continue_on_failure_step_reverts_but_operation_advances_and_succeeds() {
    let store = Arc::new(MemoryStore::new());
    let op = stored(
        true,
        None,
        vec![
            sample_transaction().continue_on_failure(true),
            sample_transaction(),
        ],
        vec![],
    );
    store.save_operation(op.clone()).await.unwrap();

    // Step 0 reverts on chain; step 1 succeeds. Both are submitted (the revert of a
    // tolerated step does not stop the operation), so both submits are consumed.
    let executor = FakeExecutor::new(
        vec![Ok(Some(step_outcome(false))), Ok(Some(step_outcome(true)))],
        vec![],
    );

    let result = driver(store.clone(), executor)
        .execute_remaining_steps(op)
        .await
        .unwrap();

    assert_eq!(result.status(), OperationStatus::Succeeded);
    assert!(result.current_step.is_none());
    assert!(result.remaining_steps.is_empty());
    assert_eq!(result.completed_steps.len(), 2);
    assert!(
        matches!(result.completed_steps[0], CompletedStep::Reverted(_)),
        "the tolerated revert must be recorded as a completed reverted step"
    );
    assert!(matches!(
        result.completed_steps[1],
        CompletedStep::Succeeded(_)
    ));
}

/// A non-`continue_on_failure` step that reverts is terminal: it fails the
/// operation and no later step runs. This is the invariant the tolerant path must
/// not weaken — proven by the executor panicking on a second, unexpected submit.
#[tokio::test]
async fn non_fallible_revert_fails_operation_and_stops() {
    let store = Arc::new(MemoryStore::new());
    let op = stored(
        true,
        None,
        vec![sample_transaction(), sample_transaction()],
        vec![],
    );
    store.save_operation(op.clone()).await.unwrap();

    // Only one submit is canned: if the driver tried to run step 1 after step 0
    // reverted, `FakeExecutor` would panic on the unexpected submit.
    let executor = FakeExecutor::new(vec![Ok(Some(step_outcome(false)))], vec![]);

    let result = driver(store.clone(), executor)
        .execute_remaining_steps(op)
        .await
        .unwrap();

    assert_eq!(result.status(), OperationStatus::Failed);
    assert!(matches!(
        result.current_step,
        Some(CurrentStep::Reverted { .. })
    ));
    assert_eq!(result.remaining_steps.len(), 1, "step 1 must not have run");
    assert!(result.completed_steps.is_empty());
}

// ---- per-key nonce serialization (ENG-530) ----

fn broadcast_driver(
    store: Arc<MemoryStore>,
    log: Arc<BroadcastLog>,
    keys: u8,
    broadcast_delay: Duration,
) -> OperationDriver {
    OperationDriver::new(
        store,
        Arc::new(FakeSigner::with_keys(keys)),
        Arc::new(BroadcastExecutor {
            log,
            broadcast_delay,
        }),
    )
}

/// A distinct single-step operation, so concurrent writers contend only on the
/// signing key and never on the per-operation lock.
async fn seed_operation(store: &Arc<MemoryStore>, index: usize) -> StoredOperation {
    let mut op = stored(true, None, vec![sample_transaction()], vec![]);
    op.id = OperationId(format!("op-{index}"));
    store.save_operation(op.clone()).await.unwrap();
    op
}

async fn drive_concurrently(
    store: &Arc<MemoryStore>,
    driver: &OperationDriver,
    writers: usize,
) -> Vec<StoredOperation> {
    let mut handles = Vec::with_capacity(writers);
    for index in 0..writers {
        let op = seed_operation(store, index).await;
        let driver = driver.clone();
        handles.push(tokio::spawn(async move {
            driver.execute_remaining_steps(op).await
        }));
    }

    let mut results = Vec::with_capacity(writers);
    for handle in handles {
        results.push(handle.await.unwrap().unwrap());
    }
    results
}

fn assert_ascending(nonces: &[u64]) {
    assert!(
        nonces.windows(2).all(|pair| pair[0] < pair[1]),
        "broadcast out of nonce order: {nonces:?}"
    );
}

/// NEAR rejects any transaction whose nonce is not above the access key's
/// current one, so concurrent writers sharing a key must broadcast in nonce
/// order. Before per-key leasing they raced and a fraction were rejected.
#[tokio::test]
async fn concurrent_writers_on_one_key_broadcast_in_nonce_order() {
    const WRITERS: usize = 8;

    let store = Arc::new(MemoryStore::new());
    let log = Arc::new(BroadcastLog::default());
    let driver = broadcast_driver(store.clone(), log.clone(), 1, Duration::from_millis(20));

    for operation in drive_concurrently(&store, &driver, WRITERS).await {
        assert_eq!(operation.status(), OperationStatus::Succeeded);
    }

    assert_eq!(log.broadcasts(), WRITERS);
    assert_eq!(
        log.max_active_per_key(),
        1,
        "one access key must never broadcast concurrently"
    );
    assert_ascending(&log.nonces_for(pool_public_key(0)));
}

/// Pooling stays a throughput multiplier: separate keys run in parallel, each
/// still in nonce order.
#[tokio::test]
async fn pooled_keys_broadcast_in_parallel() {
    const KEYS: u8 = 2;
    const WRITERS: usize = 8;

    let store = Arc::new(MemoryStore::new());
    let log = Arc::new(BroadcastLog::default());
    let driver = broadcast_driver(store.clone(), log.clone(), KEYS, Duration::from_millis(50));

    for operation in drive_concurrently(&store, &driver, WRITERS).await {
        assert_eq!(operation.status(), OperationStatus::Succeeded);
    }

    assert_eq!(log.broadcasts(), WRITERS);
    assert_eq!(log.max_active_per_key(), 1);
    assert_eq!(
        log.max_active_total(),
        usize::from(KEYS),
        "pooled keys must broadcast in parallel"
    );
    for index in 0..KEYS {
        assert_ascending(&log.nonces_for(pool_public_key(index)));
    }
}

/// A step signed in an earlier pass carries a nonce bound to the key that signed
/// it, so resubmission takes *that* key. Leasing a fresh one would let a new
/// allocation — reading an on-chain nonce this step has not yet advanced — pick
/// the same nonce.
#[tokio::test]
async fn prepared_step_resubmits_under_the_key_that_signed_it() {
    let store = Arc::new(MemoryStore::new());
    let log = Arc::new(BroadcastLog::default());
    let driver = broadcast_driver(store.clone(), log.clone(), 2, Duration::ZERO);

    let signing_key = pool_public_key(1);
    let op = stored(
        true,
        Some(CurrentStep::Prepared {
            transaction: sample_transaction(),
            signed_transaction: Box::new(dummy_signed_transaction(signing_key, 42)),
            tx_hash: CryptoHash(NearCryptoHash::default()),
        }),
        vec![],
        vec![],
    );
    store.save_operation(op.clone()).await.unwrap();

    let result = driver.execute_remaining_steps(op).await.unwrap();

    assert_eq!(result.status(), OperationStatus::Succeeded);
    assert_eq!(log.nonces_for(signing_key), vec![42]);
    assert!(
        log.nonces_for(pool_public_key(0)).is_empty(),
        "must not resign the step on a different key"
    );
}

/// A key rotated out of the configuration since the step was signed leaves the
/// stored signature valid on chain, and no lane here can allocate a nonce on
/// that key — so recovery replays it rather than failing startup.
#[tokio::test]
async fn prepared_step_replays_under_a_key_the_pool_no_longer_holds() {
    let store = Arc::new(MemoryStore::new());
    let log = Arc::new(BroadcastLog::default());
    let driver = broadcast_driver(store.clone(), log.clone(), 1, Duration::ZERO);

    let retired_key = pool_public_key(7);
    let op = stored(
        true,
        Some(CurrentStep::Prepared {
            transaction: sample_transaction(),
            signed_transaction: Box::new(dummy_signed_transaction(retired_key, 42)),
            tx_hash: CryptoHash(NearCryptoHash::default()),
        }),
        vec![],
        vec![],
    );
    store.save_operation(op.clone()).await.unwrap();

    let result = driver.execute_remaining_steps(op).await.unwrap();

    assert_eq!(result.status(), OperationStatus::Succeeded);
    assert_eq!(log.nonces_for(retired_key), vec![42]);
}
