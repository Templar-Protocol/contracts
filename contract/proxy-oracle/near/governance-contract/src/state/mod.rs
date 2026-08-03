//! Versioned on-chain state for the governance contract, mirroring the oracle and pyth-lazer
//! adapters so `migrate` behaves uniformly across contracts (JSON `Migration` selects the transform).

use near_sdk::{near, store::LookupMap, AccountId};
use templar_common::versioned_state::{StateVersion, VersionedState};
use templar_proxy_oracle_governance_kernel::Governance;
use templar_proxy_oracle_near_governance_common::{GovernancePolicy, Operation, Proposal};

use crate::{StorageKey, MAX_PENDING_PROPOSALS};

pub mod legacy;
pub mod migration;

/// Current (v1) governance state. The generic-operation restructure landed in-place on v1, since v1
/// was never released; the only released layout is the pre-restructure [`legacy`] v0.
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub header: Governance<GovernancePolicy>,
    pub proposals: LookupMap<u32, Proposal<Operation>>,
    pub proxy_oracle_id: AccountId,
}

impl StateVersion for State {
    const VERSION: u32 = 1;
    type NewArgs = (AccountId, GovernancePolicy);

    fn new((proxy_oracle_id, policy): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            header: Governance::new(policy, MAX_PENDING_PROPOSALS),
            proposals: LookupMap::new(StorageKey::Proposals),
            proxy_oracle_id,
        })
    }
}
