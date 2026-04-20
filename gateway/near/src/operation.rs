use blockchain_gateway_core::{
    operation::OperationRecord, rpc::common::TxExecutionStatus, ManagedAccountId, OperationId,
};
use near_api::types::{transaction::actions::Action, AccountId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTransaction {
    pub signer_account_id: ManagedAccountId,
    pub receiver_id: AccountId,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPlan {
    pub wait_until: TxExecutionStatus,
    pub steps: Vec<PlannedTransaction>,
}

#[derive(Debug, Clone)]
pub struct StoredOperation {
    pub request_fingerprint_hash: [u8; 32],
    #[allow(dead_code)]
    pub request_payload: Vec<u8>,
    pub plan: OperationPlan,
    pub operation: OperationRecord,
}

impl StoredOperation {
    pub fn operation_id(&self) -> &OperationId {
        &self.operation.id
    }
}
