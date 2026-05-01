//! Gateway planning interfaces, NEAR client integration, and lightweight operation semantics.

pub mod client;
mod context;
mod contract_kind;
mod error;
mod executor;
mod near_client_provider;
mod operation;
mod oracle_write;
mod payload;
mod planning;
mod read;

use async_trait::async_trait;
use templar_gateway_types::{operation::OperationId, IdempotencyKey, ManagedAccountId};

pub use client::{ContractWriteOptions, NearClient};
pub use context::{GatewayContext, GatewayContextBuilder};
pub use contract_kind::query_contract_kind;
pub use error::{GatewayError, GatewayResult};
pub use executor::{
    ExecuteOperation, NearOperationExecutor, NearTransactionSigner, SharedExecuteOperation,
    SharedSignTransaction, SignTransaction,
};
pub use near_client_provider::HasNearClient;
pub use operation::{
    CurrentStep, CurrentStepRef, OperationPlan, PendingPreparation, PlannedTransaction,
    PreparedCurrentStep, PreparedTransactionResult, SharedOperationStore, StoredOperation,
    SubmittedCurrentStep, SucceededStep,
};
pub use oracle_write::{plan_pyth_update, plan_redstone_write_prices};
pub use payload::OraclePayloadSource;
pub use planning::{DispatchRead, HasIdempotencyKey, HasSignerAccountId, PlanWrite};
pub use read::ReadNear;

pub enum CreateOperationResult {
    Created(StoredOperation),
    Existing(StoredOperation),
}

#[async_trait]
pub trait OperationStore: Send + Sync {
    async fn get_by_id(&self, operation_id: &OperationId)
        -> GatewayResult<Option<StoredOperation>>;

    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>>;

    async fn create_or_get_operation(
        &self,
        rpc_method: &str,
        signer_account_id: ManagedAccountId,
        idempotency_key: Option<IdempotencyKey>,
        request_fingerprint_hash: [u8; 32],
        request_payload: Vec<u8>,
        plan: OperationPlan,
    ) -> GatewayResult<CreateOperationResult>;

    async fn save_operation(&self, operation: StoredOperation) -> GatewayResult<()>;

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>>;
}
