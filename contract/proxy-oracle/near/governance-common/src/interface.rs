use near_sdk::{near, AccountId};
use templar_proxy_oracle_governance_kernel as kernel;

use crate::OperationKind;

pub type Proposal<T> = kernel::Proposal<T, AccountId>;
pub type Governance = kernel::Governance<crate::GovernancePolicy>;

pub use kernel::{
    CancelError, CreateError, ExecuteError, IdOutOfBoundsError, IdOutOfOrderError, OperationPolicy,
    ProposalDoesNotExistError, TtlNotElapsedError,
};

/// Governance lifecycle events. They carry the proposal id, coarse operation kind, and — for target
/// calls — the invoked `method` name, but never the full operation, whose payload (e.g. a
/// `SelfUpgrade` wasm blob) can exceed NEAR's per-log size limit. Fetch the full body via
/// `get_proposal`.
#[near(event_json(standard = "templar-governance"))]
pub enum Event {
    /// When a new proposal is created.
    #[event_version("3.0.0")]
    Created {
        id: u32,
        kind: OperationKind,
        method: Option<String>,
    },
    /// When a proposal is cancelled.
    #[event_version("3.0.0")]
    Cancelled {
        id: u32,
        kind: OperationKind,
        method: Option<String>,
    },
    /// When a proposal is executed.
    #[event_version("3.0.0")]
    Executed {
        id: u32,
        kind: OperationKind,
        method: Option<String>,
    },
}

pub trait Validatable {
    type OnCreateError;
    type OnExecuteError;

    fn on_create(&self) -> Result<(), Self::OnCreateError> {
        Ok(())
    }

    fn on_execute(&self) -> Result<(), Self::OnExecuteError> {
        Ok(())
    }
}

pub mod error {
    pub use templar_proxy_oracle_governance_kernel::{
        CancelError, CreateError, ExecuteError, IdOutOfBoundsError, IdOutOfOrderError,
        ProposalDoesNotExistError, TtlNotElapsedError,
    };
}

#[macro_export]
macro_rules! gen_ext_governance {
    ($ext_name: ident, $trait_name: ident, $operation_ty: ty) => {
        #[::near_sdk::ext_contract($ext_name)]
        pub trait $trait_name {
            fn next_proposal_id(&self) -> u32;
            fn proposal_count(&self) -> u32;
            fn list_proposals(&self, offset: Option<u32>, count: Option<u32>) -> Vec<u32>;
            fn get_proposal(&self, id: u32) -> Option<$crate::interface::Proposal<$operation_ty>>;
            fn get_effective_proposal_ttl(
                &self,
                operation: $operation_ty,
                requested_ttl: $crate::Nanoseconds,
            ) -> $crate::Nanoseconds;
            fn get_governance_policy(&self) -> $crate::GovernancePolicy;
            fn create_proposal(
                &mut self,
                id: u32,
                operation: $operation_ty,
                requested_ttl: $crate::Nanoseconds,
            ) -> $crate::interface::Proposal<$operation_ty>;
            /// Borsh-argument twin of `create_proposal`, for wasm-carrying payloads that base64-in-JSON
            /// makes too costly to parse or too large for a transaction. Returns nothing; read the
            /// stored body with `get_proposal`.
            ///
            /// Arguments decode positionally, so `(id, operation, requested_ttl)` is a wire contract.
            fn create_proposal_borsh(
                &mut self,
                #[serializer(borsh)] id: u32,
                #[serializer(borsh)] operation: $operation_ty,
                #[serializer(borsh)] requested_ttl: $crate::Nanoseconds,
            );
            fn cancel_proposal(&mut self, id: u32);
            fn execute_proposal(&mut self, id: u32);
        }
    };
}
