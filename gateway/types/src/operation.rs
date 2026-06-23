use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CryptoHash, ManagedAccountId};

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
    },
    Failed {
        tx_hash: CryptoHash,
        /// The on-chain failure reason (e.g. a contract panic message), when
        /// available. Optional and defaulted for backwards compatibility.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TransactionStepRecord {
    pub index: u32,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationRecord {
    pub id: OperationId,
    pub signer_account_id: ManagedAccountId,
    pub status: OperationStatus,
    pub steps: Vec<TransactionStepRecord>,
}

impl OperationRecord {
    /// The failure reason of the first failed step, if any (e.g. a contract
    /// panic message).
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.steps.iter().find_map(|step| match &step.status {
            StepStatus::Failed { failure, .. } => failure.as_deref(),
            _ => None,
        })
    }
}
