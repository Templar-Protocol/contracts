use borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::{
    json_types::U64,
    near,
    serde::Serialize,
    store::{iterable_map, key, IterableMap},
    AccountId, IntoStorageKey,
};

use crate::oracle::pyth::PriceIdentifier;

use super::Proxy;

#[near(event_json(standard = "templar-proxy-oracle-governance"))]
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
    pub created_at_ms: U64,
    pub created_by: AccountId,
}

impl<T> Proposal<T> {
    pub fn can_execute(&self, now_ms: u64, ttl_ms: u64) -> bool {
        now_ms.saturating_sub(self.created_at_ms.0) >= ttl_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Operation {
    SetProxy {
        id: PriceIdentifier,
        proxy: Option<Proxy>,
    },
    SetActionTtl {
        new_ttl_ms: U64,
    },
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct Governance<T: BorshSerialize> {
    pub next_id: u32,
    pub ttl_ms: u64,
    pub proposals: IterableMap<u32, Proposal<T>, key::Identity>,
}

pub mod error {
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
    #[error("TTL not yet elapsed for proposal {id}: current timestamp {now_ms} < created at {created_at_ms} + TTL {ttl_ms}")]
    pub struct TtlNotElapsedError {
        pub id: u32,
        pub now_ms: u64,
        pub created_at_ms: u64,
        pub ttl_ms: u64,
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    pub enum CancelError {
        #[error(transparent)]
        IdOutOfBounds(#[from] IdOutOfBoundsError),
        #[error(transparent)]
        ProposalDoesNotExist(#[from] ProposalDoesNotExistError),
    }

    #[derive(thiserror::Error, Debug, PartialEq, Eq)]
    pub enum ExecuteError {
        #[error(transparent)]
        IdOutOfBounds(#[from] IdOutOfBoundsError),
        #[error(transparent)]
        ProposalDoesNotExist(#[from] ProposalDoesNotExistError),
        #[error(transparent)]
        IdOutOfOrder(#[from] IdOutOfOrderError),
        #[error(transparent)]
        TtlNotElapsed(#[from] TtlNotElapsedError),
    }
}

impl<T: BorshSerialize> Governance<T> {
    pub fn new(prefix: impl IntoStorageKey) -> Self {
        Self {
            next_id: 0,
            ttl_ms: 0,
            proposals: IterableMap::with_hasher(prefix.into_storage_key()),
        }
    }
}

impl<T: Clone + Serialize + BorshSerialize + BorshDeserialize> Governance<T> {
    /// Creates a new proposal.
    ///
    /// # Errors
    ///
    /// If the `id` requested to be created is out-of-order.
    pub fn create(
        &mut self,
        id: u32,
        operation: T,
        timestamp_ms: u64,
        created_by: AccountId,
    ) -> Result<Proposal<T>, error::IdOutOfOrderError> {
        if id != self.next_id {
            return Err(error::IdOutOfOrderError {
                expected: self.next_id,
                actual: id,
            });
        }

        self.next_id += 1;

        let proposal = Proposal {
            operation,
            created_at_ms: timestamp_ms.into(),
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
    /// # use near_sdk::{env, near};
    /// # use templar_common::oracle::proxy::governance::Governance;
    /// # #[derive(Debug, Clone)]
    /// # #[near(serializers = [borsh, json])]
    /// enum Op {
    ///     Increment,
    ///     Decrement,
    /// }
    /// # let now_ms = 1000;
    ///
    /// let mut g = Governance::<Op>::new(b"g");
    /// # let id = 0;
    /// # g.create(id, Op::Increment, now_ms, "alice".parse().unwrap()).unwrap();
    ///
    /// match g.execute(id, now_ms).unwrap() {
    ///     Op::Increment => println!("Actually perform the increment operation here"),
    ///     Op::Decrement => println!("Actually perform the decrement operation here"),
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// If an `id` is out-of-bounds, does not exist, or if the proposal cannot
    /// yet be executed (TTL not elapsed).
    pub fn execute(&mut self, id: u32, now_ms: u64) -> Result<T, error::ExecuteError> {
        if id >= self.next_id {
            return Err(error::IdOutOfBoundsError {
                exclusive_maximum: self.next_id,
                actual: id,
            }
            .into());
        }

        let min = self.proposals.keys().min().copied();

        let iterable_map::Entry::Occupied(e) = self.proposals.entry(id) else {
            return Err(error::ProposalDoesNotExistError { id }.into());
        };

        let Some(min) = min else {
            // Unreachable.
            near_sdk::env::abort();
        };

        // Require that operations are executed in order (or cancelled).
        if id != min {
            return Err(error::IdOutOfOrderError {
                expected: min,
                actual: id,
            }
            .into());
        }

        let proposal = e.get();

        if proposal.can_execute(now_ms, self.ttl_ms) {
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
                now_ms,
                created_at_ms: proposal.created_at_ms.0,
                ttl_ms: self.ttl_ms,
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
            fn gov_next_id(&self) -> u32;
            fn gov_ttl_ms(&self) -> ::near_sdk::json_types::U64;
            fn gov_count(&self) -> u32;
            fn gov_list(&self, offset: Option<u32>, count: Option<u32>) -> Vec<u32>;
            fn gov_get(
                &self,
                id: u32,
            ) -> Option<$crate::oracle::proxy::governance::Proposal<$operation_ty>>;
            fn gov_create(
                &mut self,
                id: u32,
                operation: $operation_ty,
            ) -> $crate::oracle::proxy::governance::Proposal<$operation_ty>;
            fn gov_cancel(&mut self, id: u32);
            fn gov_execute(&mut self, id: u32);
        }
    };
}

gen_ext_governance!(ext_governance, GovernanceInterface, Operation);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute() {
        use near_sdk::{env, near};

        #[derive(Debug, Clone)]
        #[near(serializers = [borsh, json])]
        enum Op {
            Increment,
            Decrement,
        }

        let mut g = Governance::<Op>::new(b"g");
        g.create(
            0,
            Op::Increment,
            env::block_timestamp_ms(),
            env::predecessor_account_id(),
        )
        .unwrap();
        match g.execute(0, env::block_timestamp_ms()).unwrap() {
            Op::Increment => {
                todo!("Actually perform the increment operation here")
            }
            Op::Decrement => {
                todo!("Actually perform the decrement operation here")
            }
        }
    }

    #[test]
    fn create() {
        let alice: AccountId = "alice.near".parse().unwrap();
        let mut g = Governance::<String>::new(b"g");

        assert_eq!(
            g.create(0, "hello".into(), 0, alice.clone()).unwrap(),
            Proposal {
                operation: "hello".into(),
                created_at_ms: 0.into(),
                created_by: alice.clone(),
            },
        );

        assert_eq!(
            g.create(0, "hello 2".into(), 0, alice.clone()).unwrap_err(),
            error::IdOutOfOrderError {
                expected: 1,
                actual: 0
            },
        );

        assert_eq!(g.execute(0, 0).unwrap(), "hello");

        assert_eq!(
            g.create(0, "hello 3".into(), 0, alice.clone()).unwrap_err(),
            error::IdOutOfOrderError {
                expected: 1,
                actual: 0
            },
        );

        assert_eq!(
            g.create(1, "hello 4".into(), 0, alice.clone()).unwrap(),
            Proposal {
                operation: "hello 4".into(),
                created_at_ms: 0.into(),
                created_by: alice.clone(),
            },
        );

        assert_eq!(g.execute(1, 0).unwrap(), "hello 4");

        g.create(2, "hello 5".into(), 0, alice.clone()).unwrap();
        g.create(3, "hello 6".into(), 0, alice.clone()).unwrap();
        g.create(4, "hello 7".into(), 0, alice.clone()).unwrap();

        assert_eq!(
            g.execute(3, 0).unwrap_err(),
            error::IdOutOfOrderError {
                expected: 2,
                actual: 3
            }
            .into(),
        );

        assert_eq!(
            g.execute(4, 0).unwrap_err(),
            error::IdOutOfOrderError {
                expected: 2,
                actual: 4
            }
            .into(),
        );

        assert_eq!(
            g.execute(5, 0).unwrap_err(),
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
        g.cancel(3).unwrap();
        assert_eq!(
            g.cancel(5).unwrap_err(),
            error::IdOutOfBoundsError {
                exclusive_maximum: 5,
                actual: 5
            }
            .into(),
        );

        g.execute(2, 0).unwrap();
        assert_eq!(
            g.execute(3, 0).unwrap_err(),
            error::ProposalDoesNotExistError { id: 3 }.into(),
        );
        g.execute(4, 0).unwrap();
        assert_eq!(
            g.execute(5, 0).unwrap_err(),
            error::IdOutOfBoundsError {
                exclusive_maximum: 5,
                actual: 5
            }
            .into(),
        );
    }
}
