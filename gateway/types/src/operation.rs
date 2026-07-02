use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Base64Bytes, CryptoHash, ManagedAccountId, NearGas, NearToken};

/// Whether a single receipt executed successfully or failed.
///
/// A transaction can be a top-level success while an inner receipt fails (e.g. a
/// rejected `ft_transfer_call` whose callback panics and is refunded), so a
/// consumer that requires *every* receipt to have succeeded must check this.
/// (The failure *message* is not captured — near_api keeps per-receipt status
/// private; see ENG-407.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReceiptStatus {
    Succeeded,
    Failed,
}

/// The outcome of a single receipt: the contract that executed it, whether it
/// succeeded, and the logs it emitted.
///
/// Logs are grouped per receipt rather than flattened so that (a) consumers
/// interpreting log *content* (e.g. detecting a token transfer) can attribute it
/// to the executing contract — a transaction's receipts can run untrusted code,
/// so a flat list is spoofable — and (b) receipt boundaries are preserved,
/// including a receipt that executed but emitted no logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReceiptOutcome {
    pub contract_id: near_account_id::AccountId,
    pub status: ReceiptStatus,
    pub logs: Vec<String>,
}

/// The result of executing an operation's transaction on chain, captured from
/// the submission outcome the RPC already returns — so callers get the return
/// value, logs, and cost without a follow-up `tx.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionOutcome {
    /// Total NEAR burnt across the transaction and all its receipts — the true
    /// cost the signer paid.
    pub tokens_burnt: NearToken,
    pub total_gas_burnt: NearGas,
    /// Per-receipt outcomes (the executing contract and its logs), in order.
    pub receipts: Vec<ReceiptOutcome>,
    /// The contract call's raw return value, if any.
    pub return_value: Option<Base64Bytes>,
    /// The on-chain failure reason (e.g. a contract panic message) when the
    /// transaction reverted; `None` on success. A contract panic surfaces here,
    /// not in `receipts[].logs`, so this is the field to assert failure text on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

impl From<near_api_types::transaction::result::ExecutionFinalResult> for ExecutionOutcome {
    fn from(result: near_api_types::transaction::result::ExecutionFinalResult) -> Self {
        let tokens_burnt = result
            .outcomes()
            .iter()
            .map(|outcome| outcome.tokens_burnt)
            .fold(NearToken::from_yoctonear(0), NearToken::saturating_add);
        let total_gas_burnt = result.total_gas_burnt;
        // A receipt is failed iff `receipt_failures()` (which reads near_api's
        // private per-receipt status) returns a reference to it; compare by
        // identity since those references point into the `receipt_outcomes()`
        // slice.
        let failed: Vec<*const _> = result
            .receipt_failures()
            .iter()
            .map(|outcome| std::ptr::from_ref(*outcome))
            .collect();
        // Group logs per receipt with the executing contract, preserving receipt
        // boundaries so consumers can attribute log content safely. Excludes the
        // transaction outcome (not a receipt; it emits no logs).
        let receipts = result
            .receipt_outcomes()
            .iter()
            .map(|outcome| ReceiptOutcome {
                contract_id: outcome.executor_id.clone(),
                status: if failed.contains(&std::ptr::from_ref(outcome)) {
                    ReceiptStatus::Failed
                } else {
                    ReceiptStatus::Succeeded
                },
                logs: outcome.logs.clone(),
            })
            .collect();
        // Capture the failure reason before consuming `result`. Only clone on the
        // (rare) failure path; the private per-receipt status is unreachable, so
        // `into_result()` is the sole route to the panic/error text.
        let failure = result
            .is_failure()
            .then(|| {
                result
                    .clone()
                    .into_result()
                    .err()
                    .map(|error| error.to_string())
            })
            .flatten();
        // Consume `result` last: `raw_bytes` takes it by value.
        let return_value = result.raw_bytes().ok().map(Base64Bytes::from);
        Self {
            tokens_burnt,
            total_gas_burnt,
            receipts,
            return_value,
            failure,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct OperationId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum OperationStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StepStatus {
    NotStarted,
    Prepared {
        tx_hash: CryptoHash,
    },
    Submitted {
        tx_hash: CryptoHash,
    },
    Succeeded {
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    },
    /// Executed on chain, but the transaction's final outcome was a failure.
    /// NEAR executes asynchronously, so earlier receipts in the graph may have
    /// committed state changes before a later one failed; `outcome` records what
    /// actually ran (logs, gas burnt, return value).
    Reverted {
        tx_hash: CryptoHash,
        outcome: ExecutionOutcome,
    },
    /// Failed before a recorded on-chain execution (e.g. a submission error), so
    /// no state changed as far as the gateway observed.
    Rejected {
        tx_hash: CryptoHash,
    },
}

impl StepStatus {
    /// The transaction hash for this step, if it has reached a stage that has
    /// one (`NotStarted` has none).
    #[must_use]
    pub fn tx_hash(&self) -> Option<CryptoHash> {
        match self {
            Self::Prepared { tx_hash }
            | Self::Submitted { tx_hash }
            | Self::Succeeded { tx_hash, .. }
            | Self::Reverted { tx_hash, .. }
            | Self::Rejected { tx_hash } => Some(*tx_hash),
            Self::NotStarted => None,
        }
    }

    /// The execution outcome for this step, if it executed on chain (whether it
    /// succeeded or reverted).
    #[must_use]
    pub fn outcome(&self) -> Option<&ExecutionOutcome> {
        match self {
            Self::Succeeded { outcome, .. } | Self::Reverted { outcome, .. } => Some(outcome),
            Self::NotStarted
            | Self::Prepared { .. }
            | Self::Submitted { .. }
            | Self::Rejected { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransactionStepRecord {
    pub index: u32,
    pub status: StepStatus,
}

impl TransactionStepRecord {
    /// The transaction hash for this step, if any.
    #[must_use]
    pub fn tx_hash(&self) -> Option<CryptoHash> {
        self.status.tx_hash()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationRecord {
    pub id: OperationId,
    pub signer_account_id: ManagedAccountId,
    pub status: OperationStatus,
    pub steps: Vec<TransactionStepRecord>,
}

impl OperationRecord {
    /// The transaction hash of the highest-`index` step that has one, or `None`
    /// if no step has been prepared yet.
    ///
    /// Keyed on `index` rather than vector position so a record whose steps
    /// arrived out of order still identifies the most recent transaction.
    #[must_use]
    pub fn latest_tx_hash(&self) -> Option<CryptoHash> {
        self.steps
            .iter()
            .filter_map(|step| step.tx_hash().map(|tx_hash| (step.index, tx_hash)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, tx_hash)| tx_hash)
    }

    /// Total NEAR burnt across every step that executed on chain — the true
    /// cost of the operation, summed over its transactions.
    #[must_use]
    pub fn tokens_burnt(&self) -> NearToken {
        self.steps
            .iter()
            .filter_map(|step| step.status.outcome())
            .map(|outcome| outcome.tokens_burnt)
            .fold(NearToken::from_yoctonear(0), NearToken::saturating_add)
    }

    /// The execution outcome of the operation's last executed step, if any.
    #[must_use]
    pub fn final_outcome(&self) -> Option<&ExecutionOutcome> {
        // Keyed on `index`, not vector position, so an out-of-order `steps`
        // (see `latest_tx_hash`) still picks the most recent executed step.
        self.steps
            .iter()
            .filter_map(|step| step.status.outcome().map(|outcome| (step.index, outcome)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, outcome)| outcome)
    }

    /// The on-chain failure reason of the operation's last executed step, if it
    /// reverted (e.g. a contract panic message). `None` if it succeeded or never
    /// executed. Assert failure text on this rather than on step logs.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.final_outcome()?.failure.as_deref()
    }
}
