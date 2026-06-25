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
    Prepared { tx_hash: CryptoHash },
    Submitted { tx_hash: CryptoHash },
    Succeeded { tx_hash: CryptoHash },
    Failed { tx_hash: CryptoHash },
}

impl StepStatus {
    /// The transaction hash for this step, if it has reached a stage that has
    /// one (`NotStarted` has none).
    #[must_use]
    pub fn tx_hash(&self) -> Option<CryptoHash> {
        match self {
            Self::Prepared { tx_hash }
            | Self::Submitted { tx_hash }
            | Self::Succeeded { tx_hash }
            | Self::Failed { tx_hash } => Some(*tx_hash),
            Self::NotStarted => None,
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
    /// The transaction hash of the latest step that has one (most-recent step
    /// first), or `None` if no step has been prepared yet.
    #[must_use]
    pub fn latest_tx_hash(&self) -> Option<CryptoHash> {
        self.steps
            .iter()
            .rev()
            .find_map(TransactionStepRecord::tx_hash)
    }
}
