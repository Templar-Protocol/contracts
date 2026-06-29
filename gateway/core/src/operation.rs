use std::{collections::VecDeque, sync::Arc};

use near_api::types::{
    transaction::{actions::Action, SignedTransaction},
    AccountId,
};
use serde::{Deserialize, Serialize};
use templar_gateway_types::{
    common::TxExecutionStatus,
    operation::{ExecutionOutcome, OperationRecord},
    CryptoHash, ManagedAccountId, OperationId, OperationStatus, StepStatus,
};

use crate::{GatewayResult, OperationStore};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTransaction {
    pub signer_account_id: ManagedAccountId,
    pub wait_until: TxExecutionStatus,
    pub receiver_id: AccountId,
    pub actions: Vec<Action>,
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
        wait_until: TxExecutionStatus,
        receiver_id: AccountId,
        actions: Vec<Action>,
    ) -> Self {
        Self {
            signer_account_id,
            wait_until,
            receiver_id,
            actions,
        }
    }

    #[must_use]
    pub fn single_action(
        signer_account_id: ManagedAccountId,
        wait_until: TxExecutionStatus,
        receiver_id: AccountId,
        action: Action,
    ) -> Self {
        Self::new(signer_account_id, wait_until, receiver_id, vec![action])
    }

    #[must_use]
    pub fn with_wait_until(mut self, wait_until: TxExecutionStatus) -> Self {
        self.wait_until = wait_until;
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
    pub succeeded_steps: Vec<SucceededStep>,
    pub current_step: Option<CurrentStep>,
    pub remaining_steps: VecDeque<PlannedTransaction>,
}

pub type SharedOperationStore = Arc<dyn OperationStore>;

#[must_use]
pub struct PendingPreparation<'a> {
    operation: &'a mut StoredOperation,
    store: SharedOperationStore,
    transaction: PlannedTransaction,
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
        match &self.current_step {
            Some(CurrentStep::Reverted { .. } | CurrentStep::Rejected { .. }) => {
                OperationStatus::Failed
            }
            Some(CurrentStep::Prepared { .. } | CurrentStep::Submitted { .. }) => {
                OperationStatus::InProgress
            }
            None if self.remaining_steps.is_empty() => OperationStatus::Succeeded,
            None if self.succeeded_steps.is_empty() => OperationStatus::Pending,
            None => OperationStatus::InProgress,
        }
    }

    pub fn current_step_is_failed(&self) -> bool {
        matches!(
            self.current_step,
            Some(CurrentStep::Reverted { .. } | CurrentStep::Rejected { .. })
        )
    }

    #[must_use]
    pub fn begin_next_preparation(
        &mut self,
        store: SharedOperationStore,
    ) -> Option<PendingPreparation<'_>> {
        if self.current_step.is_some() {
            return None;
        }

        let transaction = self.remaining_steps.pop_front()?;
        let transaction = if self.remaining_steps.is_empty() {
            transaction
        } else {
            transaction.with_wait_until(TxExecutionStatus::Final)
        };

        Some(PendingPreparation {
            operation: self,
            store,
            transaction,
        })
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
            self.succeeded_steps.len()
                + self.remaining_steps.len()
                + usize::from(self.current_step.is_some()),
        );

        let mut next_index = 0_u32;
        for step in &self.succeeded_steps {
            steps.push(templar_gateway_types::TransactionStepRecord {
                index: next_index,
                status: StepStatus::Succeeded {
                    tx_hash: step.tx_hash,
                    outcome: step.outcome.clone(),
                },
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
    pub fn transaction(&self) -> &PlannedTransaction {
        &self.transaction
    }

    pub async fn finish(self, prepared: PreparedTransactionResult) -> GatewayResult<()> {
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
        self.operation.current_step = Some(CurrentStep::Submitted {
            transaction: self.transaction.clone(),
            tx_hash: self.tx_hash,
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

    pub fn wait_until(&self) -> TxExecutionStatus {
        self.transaction.wait_until
    }
}

impl SubmittedCurrentStep<'_> {
    pub fn transaction(&self) -> &PlannedTransaction {
        &self.transaction
    }

    pub async fn succeed(
        self,
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    ) -> GatewayResult<()> {
        self.operation.succeeded_steps.push(SucceededStep {
            transaction: self.transaction,
            tx_hash,
            outcome,
        });
        self.operation.current_step = None;
        self.store.save_operation(self.operation.clone()).await
    }

    /// Record the step as failed. `outcome` carries the execution result when
    /// the transaction ran and reverted, and is `None` when it failed before a
    /// recorded execution — mapping to a `Reverted` or `Rejected` step
    /// respectively.
    pub async fn fail(
        self,
        tx_hash: CryptoHash,
        outcome: Option<ExecutionOutcome>,
    ) -> GatewayResult<()> {
        self.operation.current_step = Some(match outcome {
            Some(outcome) => CurrentStep::Reverted {
                transaction: self.transaction,
                tx_hash,
                outcome,
            },
            None => CurrentStep::Rejected {
                transaction: self.transaction,
                tx_hash,
            },
        });
        self.store.save_operation(self.operation.clone()).await
    }

    pub fn tx_hash(&self) -> CryptoHash {
        self.tx_hash
    }
}
