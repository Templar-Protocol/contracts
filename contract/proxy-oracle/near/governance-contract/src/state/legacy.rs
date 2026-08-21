//! v0: the released pre-restructure layout — a flat 11-field TTL table and one typed `Operation`
//! variant per action. Read-only, during migration.

use super::{Governance, LookupMap, Proposal, StateVersion, VersionedState};
use near_sdk::{
    json_types::{Base64VecU8, U128},
    near, AccountId, Gas,
};
use templar_common::{oracle::pyth::PriceIdentifier, upgrade::UpgradeSource, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{
    LegacyHistoryMode, LegacyOperation, LegacyOperationKind, LegacyTtlConfig, Role,
};

/// The v0 operation set. Borsh-identical to [`LegacyOperation`] except `AdminUpgrade`, which held a
/// raw `code` blob (`UpgradeSource` did not exist yet), and no `SelfUpgrade` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh])]
pub enum Operation {
    SetProxy {
        id: PriceIdentifier,
        proxy: Option<Proxy<Source>>,
    },
    ConfigureCircuitBreakers {
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    },
    AddCircuitBreaker {
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    },
    RemoveCircuitBreaker {
        id: PriceIdentifier,
        breaker_id: u32,
    },
    SetManualTrip {
        id: PriceIdentifier,
        is_manually_tripped: bool,
        metadata: Option<Vec<u8>>,
    },
    Rearm {
        id: PriceIdentifier,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: LegacyHistoryMode,
    },
    SetEnforced {
        id: PriceIdentifier,
        breaker_id: u32,
        is_enforced: bool,
    },
    SetActionTtl {
        kind: LegacyOperationKind,
        new_ttl: Nanoseconds,
    },
    SetRole {
        account_id: AccountId,
        role: Role,
        set: bool,
    },
    AdminUpgrade {
        code: Base64VecU8,
        migrate_args: Base64VecU8,
    },
    AdminFunctionCall {
        method_name: String,
        args: Base64VecU8,
        attached_deposit: U128,
        gas: Gas,
    },
}

impl From<Operation> for LegacyOperation {
    fn from(operation: Operation) -> Self {
        match operation {
            Operation::SetProxy { id, proxy } => Self::SetProxy { id, proxy },
            Operation::ConfigureCircuitBreakers { id, config } => {
                Self::ConfigureCircuitBreakers { id, config }
            }
            Operation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            } => Self::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            },
            Operation::RemoveCircuitBreaker { id, breaker_id } => {
                Self::RemoveCircuitBreaker { id, breaker_id }
            }
            Operation::SetManualTrip {
                id,
                is_manually_tripped,
                metadata,
            } => Self::SetManualTrip {
                id,
                is_manually_tripped,
                metadata,
            },
            Operation::Rearm {
                id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            } => Self::Rearm {
                id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            },
            Operation::SetEnforced {
                id,
                breaker_id,
                is_enforced,
            } => Self::SetEnforced {
                id,
                breaker_id,
                is_enforced,
            },
            Operation::SetActionTtl { kind, new_ttl } => Self::SetActionTtl { kind, new_ttl },
            Operation::SetRole {
                account_id,
                role,
                set,
            } => Self::SetRole {
                account_id,
                role,
                set,
            },
            // The raw v0 blob becomes an `UpgradeSource::Code`.
            Operation::AdminUpgrade { code, migrate_args } => Self::AdminUpgrade {
                code: UpgradeSource::Code(code),
                migrate_args,
            },
            Operation::AdminFunctionCall {
                method_name,
                args,
                attached_deposit,
                gas,
            } => Self::AdminFunctionCall {
                method_name,
                args,
                attached_deposit,
                gas,
            },
        }
    }
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub header: Governance<LegacyTtlConfig>,
    pub proposals: LookupMap<u32, Proposal<Operation>>,
    pub proxy_oracle_id: AccountId,
}

impl StateVersion for State {
    const VERSION: u32 = 0;
    type NewArgs = ();

    fn new((): Self::NewArgs) -> VersionedState<Self> {
        unreachable!("v0 governance state is migration-only and never constructed fresh")
    }
}
