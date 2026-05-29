use near_sdk::{near, serde::Serialize, AccountId};
use templar_proxy_oracle_governance_kernel as kernel;

pub type Proposal<T> = kernel::Proposal<T, AccountId>;
pub type Governance = kernel::Governance<crate::TtlConfig>;

pub use kernel::{
    CancelError, CreateError, ExecuteError, IdOutOfBoundsError, IdOutOfOrderError, OperationPolicy,
    ProposalDoesNotExistError, TtlNotElapsedError,
};

#[near(event_json(standard = "templar-governance"))]
pub enum Event<T: Serialize> {
    /// When a new proposal is created.
    #[event_version("1.0.0")]
    Created { id: u32, proposal: Proposal<T> },
    /// When a proposal is cancelled.
    #[event_version("1.0.0")]
    Cancelled { id: u32, proposal: Proposal<T> },
    /// When a proposal is executed.
    #[event_version("1.0.0")]
    Executed { id: u32, proposal: Proposal<T> },
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
            fn get_operation_ttl(&self, kind: $crate::OperationKind) -> $crate::Nanoseconds;
            fn create_proposal(
                &mut self,
                id: u32,
                operation: $operation_ty,
                requested_ttl: $crate::Nanoseconds,
            ) -> $crate::interface::Proposal<$operation_ty>;
            fn cancel_proposal(&mut self, id: u32);
            fn execute_proposal(&mut self, id: u32);
        }
    };
}
