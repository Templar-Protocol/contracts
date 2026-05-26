use near_sdk::{
    borsh::{BorshDeserialize, BorshSerialize},
    near,
    serde::Serialize,
    store::{iterable_map, key, IterableMap},
    AccountId, IntoStorageKey,
};
use templar_common::Nanoseconds;

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

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proposal<T> {
    pub operation: T,
    pub created_at: Nanoseconds,
    pub ttl: Nanoseconds,
    pub created_by: AccountId,
}

impl<T> Proposal<T> {
    pub fn can_execute(&self, now: Nanoseconds) -> bool {
        now.saturating_sub(self.created_at) >= self.ttl
    }
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

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct Governance<T: BorshSerialize> {
    pub next_id: u32,
    pub proposals: IterableMap<u32, Proposal<T>, key::Identity>,
}

pub mod error {
    use templar_common::Nanoseconds;

    use super::Validatable;

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    #[error("ID is out-of-order: expected {expected}, got {actual}")]
    pub struct IdOutOfOrderError {
        pub expected: u32,
        pub actual: u32,
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    #[error("ID is out-of-bounds: exclusive maximum {exclusive_maximum}, got {actual}")]
    pub struct IdOutOfBoundsError {
        pub exclusive_maximum: u32,
        pub actual: u32,
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    #[error("The proposal does not exist because it has already been cancelled or executed: {id}")]
    pub struct ProposalDoesNotExistError {
        pub id: u32,
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    #[error("TTL not yet elapsed for proposal {id}: current timestamp {now} < created at {created_at} + TTL {ttl}")]
    pub struct TtlNotElapsedError {
        pub id: u32,
        pub now: Nanoseconds,
        pub created_at: Nanoseconds,
        pub ttl: Nanoseconds,
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    pub enum CreateError<T: Validatable> {
        #[error(transparent)]
        IdOutOfOrder(#[from] IdOutOfOrderError),
        #[error("Validation error: {0}")]
        Validation(#[source] T::OnCreateError),
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    pub enum CancelError {
        #[error(transparent)]
        IdOutOfBounds(#[from] IdOutOfBoundsError),
        #[error(transparent)]
        ProposalDoesNotExist(#[from] ProposalDoesNotExistError),
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    pub enum ExecuteError<T: Validatable> {
        #[error(transparent)]
        IdOutOfBounds(#[from] IdOutOfBoundsError),
        #[error(transparent)]
        ProposalDoesNotExist(#[from] ProposalDoesNotExistError),
        #[error(transparent)]
        IdOutOfOrder(#[from] IdOutOfOrderError),
        #[error(transparent)]
        TtlNotElapsed(#[from] TtlNotElapsedError),
        #[error("Validation error: {0}")]
        Validation(#[source] T::OnExecuteError),
    }
}

impl<T: BorshSerialize> Governance<T> {
    pub fn new(prefix: impl IntoStorageKey) -> Self {
        Self {
            next_id: 0,
            proposals: IterableMap::with_hasher(prefix.into_storage_key()),
        }
    }
}

impl<T: Clone + Serialize + BorshSerialize + BorshDeserialize + Validatable> Governance<T> {
    /// Creates a new proposal.
    ///
    /// # Errors
    ///
    /// If the `id` requested to be created is out-of-order.
    pub fn create(
        &mut self,
        id: u32,
        operation: T,
        now: Nanoseconds,
        created_by: AccountId,
        ttl: Nanoseconds,
    ) -> Result<Proposal<T>, error::CreateError<T>> {
        if id != self.next_id {
            return Err(error::IdOutOfOrderError {
                expected: self.next_id,
                actual: id,
            }
            .into());
        }

        operation
            .on_create()
            .map_err(error::CreateError::Validation)?;

        self.next_id += 1;

        let proposal = Proposal {
            operation,
            created_at: now,
            ttl,
            created_by,
        };

        self.proposals.insert(id, proposal.clone());

        Event::Created {
            id,
            proposal: proposal.clone(),
        }
        .emit();

        Ok(proposal)
    }

    /// Cancels a proposal.
    ///
    /// # Errors
    ///
    /// If the `id` requested to be cancelled is out of bounds or does not exist.
    pub fn cancel(&mut self, id: u32) -> Result<(), error::CancelError> {
        if id >= self.next_id {
            return Err(error::IdOutOfBoundsError {
                exclusive_maximum: self.next_id,
                actual: id,
            }
            .into());
        }

        if let Some(proposal) = self.proposals.remove(&id) {
            Event::Cancelled { id, proposal }.emit();
            Ok(())
        } else {
            Err(error::ProposalDoesNotExistError { id }.into())
        }
    }

    /// Executes a proposal.
    ///
    /// This function simply removes the proposal from storage if it is
    /// eligible for execution and returns its associated operation. It is up
    /// to the caller to actually execute the returned operation.
    ///
    /// ```rust
    /// # use near_sdk::{near, AccountId};
    /// # use templar_common::Nanoseconds;
    /// # use templar_proxy_oracle_near_governance_common::{Governance, Validatable};
    /// # #[derive(Debug, Clone)]
    /// # #[near(serializers = [borsh, json])]
    /// enum Op {
    ///     Increment,
    ///     Decrement,
    /// }
    /// # impl Validatable for Op {
    /// #     type OnCreateError = ();
    /// #     type OnExecuteError = ();
    /// # }
    /// # let now = Nanoseconds::from_secs(1);
    /// # let created_by: AccountId = "alice.near".parse().unwrap();
    ///
    /// let mut g = Governance::<Op>::new(b"g");
    /// # let id = 0;
    /// # g.create(id, Op::Increment, now, created_by, Nanoseconds::zero()).unwrap();
    ///
    /// match g.execute(id, now).unwrap() {
    ///     Op::Increment => println!("Actually perform the increment operation here"),
    ///     Op::Decrement => println!("Actually perform the decrement operation here"),
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// If an `id` is out-of-bounds, does not exist, or if the proposal cannot
    /// yet be executed (TTL not elapsed).
    pub fn execute(&mut self, id: u32, now: Nanoseconds) -> Result<T, error::ExecuteError<T>> {
        if id >= self.next_id {
            return Err(error::IdOutOfBoundsError {
                exclusive_maximum: self.next_id,
                actual: id,
            }
            .into());
        }

        let iterable_map::Entry::Occupied(e) = self.proposals.entry(id) else {
            return Err(error::ProposalDoesNotExistError { id }.into());
        };

        let proposal = e.get();
        proposal
            .operation
            .on_execute()
            .map_err(error::ExecuteError::Validation)?;

        if proposal.can_execute(now) {
            let proposal = proposal.clone();
            e.remove();

            Event::Executed {
                id,
                proposal: proposal.clone(),
            }
            .emit();
            Ok(proposal.operation)
        } else {
            Err(error::TtlNotElapsedError {
                id,
                now,
                created_at: proposal.created_at,
                ttl: proposal.ttl,
            }
            .into())
        }
    }
}

#[macro_export]
macro_rules! gen_ext_governance {
    ($ext_name: ident, $trait_name: ident, $operation_ty: ty) => {
        #[::near_sdk::ext_contract($ext_name)]
        pub trait $trait_name {
            fn next_proposal_id(&self) -> u32;
            fn proposal_count(&self) -> u32;
            fn list_proposals(&self, offset: Option<u32>, count: Option<u32>) -> Vec<u32>;
            fn get_proposal(&self, id: u32) -> Option<$crate::engine::Proposal<$operation_ty>>;
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
            ) -> $crate::engine::Proposal<$operation_ty>;
            fn cancel_proposal(&mut self, id: u32);
            fn execute_proposal(&mut self, id: u32);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    #[near(serializers = [json, borsh])]
    struct Op(String);
    impl Validatable for Op {
        type OnCreateError = ();
        type OnExecuteError = ();

        fn on_create(&self) -> Result<(), Self::OnCreateError> {
            Ok(())
        }

        fn on_execute(&self) -> Result<(), Self::OnExecuteError> {
            if self.0.len() < 10 {
                Ok(())
            } else {
                Err(())
            }
        }
    }

    impl From<&str> for Op {
        fn from(value: &str) -> Self {
            Self(value.to_string())
        }
    }

    #[test]
    fn create() {
        let alice: AccountId = "alice.near".parse().unwrap();
        let mut g = Governance::<Op>::new(b"g");
        let now = Nanoseconds::from_ms(12345);

        assert_eq!(
            g.create(0, "hello".into(), now, alice.clone(), Nanoseconds::zero())
                .unwrap(),
            Proposal {
                operation: "hello".into(),
                created_at: now,
                ttl: Nanoseconds::zero(),
                created_by: alice.clone(),
            },
        );

        assert_eq!(
            g.create(0, "hello 2".into(), now, alice.clone(), Nanoseconds::zero())
                .unwrap_err(),
            error::IdOutOfOrderError {
                expected: 1,
                actual: 0
            }
            .into(),
        );

        assert_eq!(g.execute(0, now).unwrap(), "hello".into());

        assert_eq!(
            g.create(0, "hello 3".into(), now, alice.clone(), Nanoseconds::zero())
                .unwrap_err(),
            error::IdOutOfOrderError {
                expected: 1,
                actual: 0
            }
            .into(),
        );

        assert_eq!(
            g.create(1, "hello 4".into(), now, alice.clone(), Nanoseconds::zero())
                .unwrap(),
            Proposal {
                operation: "hello 4".into(),
                created_at: now,
                ttl: Nanoseconds::zero(),
                created_by: alice.clone(),
            },
        );

        assert_eq!(g.execute(1, now).unwrap(), "hello 4".into());

        g.create(2, "hello 5".into(), now, alice.clone(), Nanoseconds::zero())
            .unwrap();
        g.create(3, "hello 6".into(), now, alice.clone(), Nanoseconds::zero())
            .unwrap();
        g.create(4, "hello 7".into(), now, alice.clone(), Nanoseconds::zero())
            .unwrap();

        // Unordered execution: can execute 3 and 4 before 2
        assert_eq!(g.execute(3, now).unwrap(), "hello 6".into());
        assert_eq!(g.execute(4, now).unwrap(), "hello 7".into());

        assert_eq!(
            g.execute(5, now).unwrap_err(),
            error::IdOutOfBoundsError {
                exclusive_maximum: 5,
                actual: 5
            }
            .into(),
        );

        assert_eq!(
            g.cancel(0).unwrap_err(),
            error::ProposalDoesNotExistError { id: 0 }.into(),
        );
        g.cancel(2).unwrap();
        assert_eq!(
            g.cancel(5).unwrap_err(),
            error::IdOutOfBoundsError {
                exclusive_maximum: 5,
                actual: 5
            }
            .into(),
        );

        assert_eq!(
            g.execute(2, now).unwrap_err(),
            error::ProposalDoesNotExistError { id: 2 }.into(),
        );
        assert_eq!(
            g.execute(3, now).unwrap_err(),
            error::ProposalDoesNotExistError { id: 3 }.into(),
        );
        assert_eq!(
            g.execute(5, now).unwrap_err(),
            error::IdOutOfBoundsError {
                exclusive_maximum: 5,
                actual: 5
            }
            .into(),
        );
    }
}
