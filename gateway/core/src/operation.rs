use std::{collections::VecDeque, sync::Arc};

use chrono::{DateTime, Utc};
use near_api::types::{
    transaction::{actions::Action, SignedTransaction},
    AccountId,
};
use serde::{Deserialize, Serialize};
use templar_gateway_types::{
    operation::{ExecutionOutcome, OperationRecord},
    CryptoHash, ManagedAccountId, OperationId, OperationStatus, StepStatus,
};

use crate::{GatewayResult, OperationStore};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTransaction {
    pub signer_account_id: ManagedAccountId,
    pub receiver_id: AccountId,
    pub actions: Vec<Action>,
    /// When `true`, an on-chain revert of this step is *recorded and tolerated*:
    /// the operation advances to the next step instead of aborting. Used for
    /// independent steps whose failure must not cancel the rest (e.g. one oracle
    /// update among several). Defaults to `false` — steps are all-or-nothing.
    pub continue_on_failure: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedTransactionResult {
    pub transaction: PlannedTransaction,
    pub tx_hash: CryptoHash,
    pub signed_transaction: SignedTransaction,
}

impl PlannedTransaction {
    #[must_use]
    pub fn new(
        signer_account_id: ManagedAccountId,
        receiver_id: AccountId,
        actions: Vec<Action>,
    ) -> Self {
        Self {
            signer_account_id,
            receiver_id,
            actions,
            continue_on_failure: false,
        }
    }

    #[must_use]
    pub fn single_action(
        signer_account_id: ManagedAccountId,
        receiver_id: AccountId,
        action: Action,
    ) -> Self {
        Self::new(signer_account_id, receiver_id, vec![action])
    }

    /// Mark this step as tolerant of an on-chain revert: on failure the operation
    /// records the revert and advances to the next step rather than aborting.
    #[must_use]
    pub fn continue_on_failure(mut self, continue_on_failure: bool) -> Self {
        self.continue_on_failure = continue_on_failure;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPlan {
    pub steps: Vec<PlannedTransaction>,
}

impl OperationPlan {
    #[must_use]
    pub fn single(step: PlannedTransaction) -> Self {
        Self { steps: vec![step] }
    }

    /// A single-step plan. Collapses the common
    /// `OperationPlan::single(PlannedTransaction { … })` boilerplate in
    /// `PlanWrite` impls.
    #[must_use]
    pub fn execute(
        signer_account_id: ManagedAccountId,
        receiver_id: AccountId,
        actions: Vec<Action>,
    ) -> Self {
        Self::single(PlannedTransaction::new(
            signer_account_id,
            receiver_id,
            actions,
        ))
    }

    pub fn push(&mut self, step: PlannedTransaction) {
        self.steps.push(step);
    }
}

impl From<PlannedTransaction> for OperationPlan {
    fn from(step: PlannedTransaction) -> Self {
        Self::single(step)
    }
}

#[derive(Debug, Clone)]
pub struct SucceededStep {
    pub transaction: PlannedTransaction,
    pub tx_hash: CryptoHash,
    pub outcome: ExecutionOutcome,
}

/// A step that executed on chain and reverted, but was *tolerated* because it was
/// [`PlannedTransaction::continue_on_failure`]. Unlike a
/// [`CurrentStep::Reverted`] — which is terminal — a tolerated revert advances
/// the operation, so it lives among the completed steps rather than blocking.
#[derive(Debug, Clone)]
pub struct RevertedStep {
    pub transaction: PlannedTransaction,
    pub tx_hash: CryptoHash,
    pub outcome: ExecutionOutcome,
}

/// A step the operation has advanced past: either it succeeded, or it reverted
/// and was tolerated. Modelling this as an enum (rather than a `reverted: bool`)
/// makes a "reverted success" unrepresentable, and keeps the invariant that a
/// [`CurrentStep::Reverted`]/[`CurrentStep::Rejected`] means *exclusively* a
/// terminal, non-tolerated failure.
#[derive(Debug, Clone)]
pub enum CompletedStep {
    Succeeded(SucceededStep),
    Reverted(RevertedStep),
}

#[derive(Debug, Clone)]
pub enum CurrentStep {
    Prepared {
        transaction: PlannedTransaction,
        signed_transaction: Box<SignedTransaction>,
        tx_hash: CryptoHash,
    },
    Submitted {
        transaction: PlannedTransaction,
        tx_hash: CryptoHash,
        /// When the step was submitted, used to age out a transaction the chain
        /// never records into a rejection. Stamped once at submission and
        /// preserved across reconciliation sweeps.
        submitted_at: DateTime<Utc>,
    },
    /// The transaction executed on chain but its final outcome was a failure.
    Reverted {
        transaction: PlannedTransaction,
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    },
    /// The step failed before a recorded on-chain execution (e.g. a submission
    /// error).
    Rejected {
        transaction: PlannedTransaction,
        tx_hash: CryptoHash,
    },
}

#[derive(Debug, Clone)]
pub struct StoredOperation {
    pub rpc_method: String,
    pub request_fingerprint_hash: [u8; 32],
    pub request_payload: Vec<u8>,
    pub id: OperationId,
    pub signer_account_id: ManagedAccountId,
    /// Whether planning has completed. A reservation (created to hold the
    /// idempotency key before planning) is `false`; it flips to `true` once a
    /// plan is attached — including an empty (no-op) plan, so a no-op is a real
    /// terminal operation rather than being indistinguishable from a reservation.
    pub planned: bool,
    /// Steps the operation has advanced past, in execution order — successes and
    /// tolerated reverts (see [`CompletedStep`]). Everything here is behind the
    /// cursor; `current_step` is at it; `remaining_steps` are ahead.
    pub completed_steps: Vec<CompletedStep>,
    pub current_step: Option<CurrentStep>,
    pub remaining_steps: VecDeque<PlannedTransaction>,
}

pub type SharedOperationStore = Arc<dyn OperationStore>;

/// The right to install a freshly signed step. Holds no transaction: ownership
/// passes to the caller to sign, and the operation is not touched until
/// [`PendingPreparation::finish`].
#[must_use]
pub struct PendingPreparation<'a> {
    operation: &'a mut StoredOperation,
    store: SharedOperationStore,
    /// Whether the step is still queued in `remaining_steps` and so must be
    /// dequeued on completion, rather than already occupying the cursor.
    queued: bool,
}

#[must_use]
pub struct PreparedCurrentStep<'a> {
    operation: &'a mut StoredOperation,
    store: SharedOperationStore,
    transaction: PlannedTransaction,
    signed_transaction: SignedTransaction,
    tx_hash: CryptoHash,
}

#[must_use]
pub struct SubmittedCurrentStep<'a> {
    operation: &'a mut StoredOperation,
    store: SharedOperationStore,
    transaction: PlannedTransaction,
    tx_hash: CryptoHash,
}

pub enum CurrentStepRef<'a> {
    Prepared(Box<PreparedCurrentStep<'a>>),
    Submitted(Box<SubmittedCurrentStep<'a>>),
    Failed,
}

impl StoredOperation {
    pub fn operation_id(&self) -> &OperationId {
        &self.id
    }

    pub fn record(&self) -> OperationRecord {
        OperationRecord {
            id: self.id.clone(),
            signer_account_id: self.signer_account_id.clone(),
            status: self.status(),
            steps: self.transaction_step_records(),
        }
    }

    pub fn status(&self) -> OperationStatus {
        // A reservation (planning not yet complete) is never terminal, whatever
        // its steps. Once planned, status derives purely from the steps -- so a
        // planned operation with no steps is a (terminal) no-op success.
        if !self.planned {
            return OperationStatus::Pending;
        }
        match &self.current_step {
            Some(CurrentStep::Reverted { .. } | CurrentStep::Rejected { .. }) => {
                OperationStatus::Failed
            }
            Some(CurrentStep::Prepared { .. } | CurrentStep::Submitted { .. }) => {
                OperationStatus::InProgress
            }
            None if self.remaining_steps.is_empty() => OperationStatus::Succeeded,
            None if self.completed_steps.is_empty() => OperationStatus::Pending,
            None => OperationStatus::InProgress,
        }
    }

    pub fn current_step_is_failed(&self) -> bool {
        matches!(
            self.current_step,
            Some(CurrentStep::Reverted { .. } | CurrentStep::Rejected { .. })
        )
    }

    /// A bare reservation: created to hold the idempotency key before planning,
    /// with planning not yet complete. Nothing was submitted, so there is
    /// nothing on chain to resume — recovery and pre-submit error cleanup drop
    /// these. A planned no-op (`planned`, no steps) is *not* a reservation.
    pub fn is_reservation(&self) -> bool {
        !self.planned
    }

    /// Record `transaction` as a completed success and advance the cursor
    /// (clear `current_step`). Shared by live execution and reconciliation.
    pub fn record_success(
        &mut self,
        transaction: PlannedTransaction,
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    ) {
        self.completed_steps
            .push(CompletedStep::Succeeded(SucceededStep {
                transaction,
                tx_hash,
                outcome,
            }));
        self.current_step = None;
    }

    /// Record an on-chain revert of `transaction`. The transaction's own
    /// [`PlannedTransaction::continue_on_failure`] flag is the *sole* authority on
    /// whether the revert is tolerated — advancing the cursor as a completed
    /// [`CompletedStep::Reverted`] — or terminal — parking it as the failed
    /// `current_step`. Centralising the decision here is what upholds the
    /// invariant that a `CurrentStep::Reverted`/`Rejected` means exclusively a
    /// non-tolerated failure: no caller can put a tolerated revert there, and none
    /// can drop a non-tolerated one. Shared by live execution, reconciliation, and
    /// (via the persisted flag) reload.
    pub fn record_revert(
        &mut self,
        transaction: PlannedTransaction,
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    ) {
        if transaction.continue_on_failure {
            self.completed_steps
                .push(CompletedStep::Reverted(RevertedStep {
                    transaction,
                    tx_hash,
                    outcome,
                }));
            self.current_step = None;
        } else {
            self.current_step = Some(CurrentStep::Reverted {
                transaction,
                tx_hash,
                outcome,
            });
        }
    }

    /// Record `transaction` as having failed without a recorded on-chain
    /// execution. Unlike a revert, this is unconditionally terminal:
    /// [`CompletedStep`] has no rejected variant, so a rejection cannot be
    /// tolerated by [`PlannedTransaction::continue_on_failure`]. Carrying no
    /// [`ExecutionOutcome`] is what releases the step's charge.
    pub fn record_rejection(&mut self, transaction: PlannedTransaction, tx_hash: CryptoHash) {
        self.current_step = Some(CurrentStep::Rejected {
            transaction,
            tx_hash,
        });
    }

    /// Take the next step's transaction to be signed, replacing the signature of
    /// one already `Prepared` rather than reusing it. Nothing moves until
    /// [`PendingPreparation::finish`].
    ///
    /// A stored signature is bound to a block hash NEAR only accepts for ~24h,
    /// after which resubmitting it yields `Expired` forever. This is the sole
    /// route to a submittable [`PreparedCurrentStep`], which is what makes a
    /// stale signature unreachable rather than merely discouraged. Dropping it
    /// cannot double-submit: [`PreparedCurrentStep::submit`] records `Submitted`
    /// *before* broadcasting, so a persisted `Prepared` step never reached the
    /// network.
    #[must_use]
    pub fn begin_next_preparation(
        &mut self,
        store: SharedOperationStore,
    ) -> Option<(PendingPreparation<'_>, PlannedTransaction)> {
        let transaction = self.next_transaction()?.clone();
        let queued = self.current_step.is_none();

        Some((
            PendingPreparation {
                operation: self,
                store,
                queued,
            },
            transaction,
        ))
    }

    /// The next step to sign, wherever it currently sits. Single source of truth
    /// for [`begin_next_preparation`](Self::begin_next_preparation) and
    /// [`signing_target`](Self::signing_target), so they cannot disagree.
    fn next_transaction(&self) -> Option<&PlannedTransaction> {
        match &self.current_step {
            Some(CurrentStep::Prepared { transaction, .. }) => Some(transaction),
            Some(_) => None,
            None => self.remaining_steps.front(),
        }
    }

    /// The account the next step must sign under, or `None` when no step is
    /// ready to sign. Any of that account's keys will do: every step named here
    /// is signed afresh, so none is bound to the key that signed an earlier
    /// attempt.
    pub(crate) fn signing_target(&self) -> Option<&ManagedAccountId> {
        self.next_transaction()
            .map(|transaction| &transaction.signer_account_id)
    }

    #[must_use]
    pub fn current(&mut self, store: SharedOperationStore) -> Option<CurrentStepRef<'_>> {
        match self.current_step.clone() {
            Some(CurrentStep::Prepared {
                transaction,
                signed_transaction,
                tx_hash,
            }) => Some(CurrentStepRef::Prepared(Box::new(PreparedCurrentStep {
                operation: self,
                store,
                transaction,
                signed_transaction: *signed_transaction,
                tx_hash,
            }))),
            Some(CurrentStep::Submitted {
                transaction,
                tx_hash,
                submitted_at: _,
            }) => Some(CurrentStepRef::Submitted(Box::new(SubmittedCurrentStep {
                operation: self,
                store,
                transaction,
                tx_hash,
            }))),
            Some(CurrentStep::Reverted { .. } | CurrentStep::Rejected { .. }) => {
                Some(CurrentStepRef::Failed)
            }
            None => None,
        }
    }

    fn transaction_step_records(&self) -> Vec<templar_gateway_types::TransactionStepRecord> {
        let mut steps = Vec::with_capacity(
            self.completed_steps.len()
                + self.remaining_steps.len()
                + usize::from(self.current_step.is_some()),
        );

        let mut next_index = 0_u32;
        for step in &self.completed_steps {
            let status = match step {
                CompletedStep::Succeeded(step) => StepStatus::Succeeded {
                    tx_hash: step.tx_hash,
                    outcome: step.outcome.clone(),
                },
                CompletedStep::Reverted(step) => StepStatus::Reverted {
                    tx_hash: step.tx_hash,
                    outcome: step.outcome.clone(),
                },
            };
            steps.push(templar_gateway_types::TransactionStepRecord {
                index: next_index,
                status,
            });
            next_index = next_index.saturating_add(1);
        }

        if let Some(current) = &self.current_step {
            let status = match current {
                CurrentStep::Prepared { tx_hash, .. } => StepStatus::Prepared { tx_hash: *tx_hash },
                CurrentStep::Submitted { tx_hash, .. } => {
                    StepStatus::Submitted { tx_hash: *tx_hash }
                }
                CurrentStep::Reverted {
                    tx_hash, outcome, ..
                } => StepStatus::Reverted {
                    tx_hash: *tx_hash,
                    outcome: outcome.clone(),
                },
                CurrentStep::Rejected { tx_hash, .. } => StepStatus::Rejected { tx_hash: *tx_hash },
            };
            steps.push(templar_gateway_types::TransactionStepRecord {
                index: next_index,
                status,
            });
            next_index = next_index.saturating_add(1);
        }

        for transaction in &self.remaining_steps {
            let _ = transaction;
            steps.push(templar_gateway_types::TransactionStepRecord {
                index: next_index,
                status: StepStatus::NotStarted,
            });
            next_index = next_index.saturating_add(1);
        }

        steps
    }
}

impl PendingPreparation<'_> {
    /// Install the signature and advance the cursor onto the step, together. A
    /// preparation abandoned before this — a signing failure, an early return —
    /// leaves the operation exactly as it was found.
    pub async fn finish(self, prepared: PreparedTransactionResult) -> GatewayResult<()> {
        if self.queued {
            self.operation.remaining_steps.pop_front();
        }
        self.operation.current_step = Some(CurrentStep::Prepared {
            transaction: prepared.transaction,
            signed_transaction: Box::new(prepared.signed_transaction),
            tx_hash: prepared.tx_hash,
        });
        self.store.save_operation(self.operation.clone()).await
    }
}

impl<'a> PreparedCurrentStep<'a> {
    pub async fn submit(self) -> GatewayResult<(SignedTransaction, SubmittedCurrentStep<'a>)> {
        let submitted_at = Utc::now();
        self.operation.current_step = Some(CurrentStep::Submitted {
            transaction: self.transaction.clone(),
            tx_hash: self.tx_hash,
            submitted_at,
        });
        self.store.save_operation(self.operation.clone()).await?;
        Ok((
            self.signed_transaction,
            SubmittedCurrentStep {
                operation: self.operation,
                store: self.store,
                transaction: self.transaction,
                tx_hash: self.tx_hash,
            },
        ))
    }
}

impl SubmittedCurrentStep<'_> {
    pub fn transaction(&self) -> &PlannedTransaction {
        &self.transaction
    }

    pub async fn mark_succeeded(
        self,
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    ) -> GatewayResult<()> {
        self.operation
            .record_success(self.transaction, tx_hash, outcome);
        self.store.save_operation(self.operation.clone()).await
    }

    /// Record the step as having executed on chain but reverted (final outcome
    /// was a failure). Whether that reverts the operation or is tolerated is
    /// decided by [`StoredOperation::record_revert`] from the step's own flag.
    pub async fn mark_reverted(
        self,
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    ) -> GatewayResult<()> {
        self.operation
            .record_revert(self.transaction, tx_hash, outcome);
        self.store.save_operation(self.operation.clone()).await
    }

    pub fn tx_hash(&self) -> CryptoHash {
        self.tx_hash
    }
}
