//! Versioned on-chain state for the governance contract, mirroring the oracle and pyth-lazer
//! adapters so `migrate` behaves uniformly across contracts (JSON `Migration` selects the transform).

use near_sdk::{near, store::LookupMap, AccountId};
use templar_common::versioned_state::{StateVersion, VersionedState};
use templar_proxy_oracle_governance_kernel::Governance;
use templar_proxy_oracle_near_governance_common::{Operation, Proposal, TtlConfig};

use crate::{StorageKey, MAX_PENDING_PROPOSALS};

/// Current (v1) governance state: the kernel ledger header, the proposal bodies, and the governed
/// oracle account.
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub header: Governance<TtlConfig>,
    pub proposals: LookupMap<u32, Proposal<Operation>>,
    pub proxy_oracle_id: AccountId,
}

impl StateVersion for State {
    const VERSION: u32 = 1;
    type NewArgs = (AccountId, TtlConfig);

    fn new((proxy_oracle_id, ttls): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            header: Governance::new(ttls, MAX_PENDING_PROPOSALS),
            proposals: LookupMap::new(StorageKey::Proposals),
            proxy_oracle_id,
        })
    }
}

pub mod legacy {
    //! v0: the pre-standardized-upgrade layout. Its `TtlConfig` predates the `self_upgrade` field and
    //! its `Operation::AdminUpgrade` carried a raw `code` blob (there was no `SelfUpgrade`). Only used
    //! to read the old on-chain state during migration.
    use super::{Governance, LookupMap, Proposal, StateVersion, VersionedState};
    use near_sdk::{
        json_types::{Base64VecU8, U128},
        near, AccountId, Gas,
    };
    use templar_common::{oracle::pyth::PriceIdentifier, upgrade::UpgradeSource, Nanoseconds};
    use templar_proxy_oracle_kernel::proxy::{
        circuit_breaker::{AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig},
        Proxy,
    };
    use templar_proxy_oracle_near_common::input::Source;
    use templar_proxy_oracle_near_governance_common::{OperationKind, Role};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[near(serializers = [borsh])]
    pub struct TtlConfigV0 {
        pub set_proxy: Nanoseconds,
        pub configure_circuit_breakers: Nanoseconds,
        pub add_circuit_breaker: Nanoseconds,
        pub remove_circuit_breaker: Nanoseconds,
        pub set_manual_trip: Nanoseconds,
        pub rearm: Nanoseconds,
        pub set_enforced: Nanoseconds,
        pub set_action_ttl: Nanoseconds,
        pub set_role: Nanoseconds,
        pub admin_upgrade: Nanoseconds,
        pub admin_function_call: Nanoseconds,
    }

    /// The v0 operation set. Every variant is borsh-identical to the current [`super::Operation`]
    /// except `AdminUpgrade`, which held a raw `code` blob; `SelfUpgrade` did not yet exist.
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
            accepted_history_source: AcceptedHistorySource,
        },
        SetEnforced {
            id: PriceIdentifier,
            breaker_id: u32,
            is_enforced: bool,
        },
        SetActionTtl {
            kind: OperationKind,
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

    impl From<Operation> for super::Operation {
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
                // The one reshaped variant: the raw blob becomes an `UpgradeSource::Code`.
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
        pub header: Governance<TtlConfigV0>,
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
}

pub mod migration {
    use super::{legacy, Governance, LookupMap, Operation, Proposal, State, TtlConfig};
    use crate::StorageKey;
    use near_sdk::near;
    use templar_common::{
        panic_with_message,
        versioned_state::{Migrator, StateTransformer},
    };

    /// v0 → v1: add the `self_upgrade` timelock (defaulted to the existing `admin_upgrade` timelock —
    /// self-upgrading governance is at least as sensitive as upgrading the oracle) and rewrite every
    /// pending proposal body from the old `Operation` layout to the new one (`AdminUpgrade`'s raw
    /// `code` blob becomes an `UpgradeSource::Code`). Bodies are rewritten in place under the same
    /// storage prefix, so nothing is left unreadable and pending proposals stay executable.
    #[derive(Clone, Debug)]
    #[near(serializers = [json])]
    pub struct V0ToV1;

    impl StateTransformer for V0ToV1 {
        type Input = legacy::State;
        type Output = State;
        type Error = ();

        fn transform(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
            let legacy::State {
                header,
                proposals: mut old_proposals,
                proxy_oracle_id,
            } = input;

            let ttls = TtlConfig {
                set_proxy: header.ttls.set_proxy,
                configure_circuit_breakers: header.ttls.configure_circuit_breakers,
                add_circuit_breaker: header.ttls.add_circuit_breaker,
                remove_circuit_breaker: header.ttls.remove_circuit_breaker,
                set_manual_trip: header.ttls.set_manual_trip,
                rearm: header.ttls.rearm,
                set_enforced: header.ttls.set_enforced,
                set_action_ttl: header.ttls.set_action_ttl,
                set_role: header.ttls.set_role,
                admin_upgrade: header.ttls.admin_upgrade,
                admin_function_call: header.ttls.admin_function_call,
                self_upgrade: header.ttls.admin_upgrade,
            };

            // `active_ids` enumerates exactly the stored proposals. `remove` reads each body under the
            // v0 layout (correct deserialization) and clears its storage key; converted bodies are
            // collected, then `old_proposals` is dropped so the deletions flush *before* the new map
            // reuses the same prefix. This avoids the new-typed map reading stale v0 bytes.
            let migrated: Vec<(u32, Proposal<Operation>)> = header
                .active_ids
                .iter()
                .filter_map(|&id| {
                    let id = u32::try_from(id).ok()?;
                    let old = old_proposals.remove(&id)?;
                    Some((
                        id,
                        Proposal {
                            operation: Operation::from(old.operation),
                            created_at: old.created_at,
                            ttl: old.ttl,
                            created_by: old.created_by,
                        },
                    ))
                })
                .collect();
            drop(old_proposals);

            let mut proposals = LookupMap::new(StorageKey::Proposals);
            for (id, proposal) in migrated {
                proposals.insert(id, proposal);
            }

            Ok(State {
                header: Governance {
                    next_id: header.next_id,
                    active_ids: header.active_ids,
                    ttls,
                    max_pending_proposals: header.max_pending_proposals,
                },
                proposals,
                proxy_oracle_id,
            })
        }
    }

    /// JSON-tagged by `from_version`, so `migrate_args` of `{"from_version":"v0"}` selects [`V0ToV1`].
    #[derive(Clone, Debug)]
    #[near(serializers = [json])]
    #[serde(tag = "from_version", rename_all = "snake_case")]
    pub enum Migration {
        V0(V0ToV1),
    }

    impl From<V0ToV1> for Migration {
        fn from(value: V0ToV1) -> Self {
            Self::V0(value)
        }
    }

    impl Migrator for Migration {
        fn run(self) {
            match self {
                Migration::V0(v0) => {
                    v0.run().unwrap_or_else(|e| {
                        panic_with_message(&format!("Failed to migrate V0: {e}"))
                    });
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::legacy::{self, TtlConfigV0};
        use super::super::{Operation, Proposal};
        use super::V0ToV1;
        use crate::StorageKey;
        use near_sdk::json_types::Base64VecU8;
        use near_sdk::{store::LookupMap, test_utils::VMContextBuilder, testing_env};
        use templar_common::upgrade::UpgradeSource;
        use templar_common::versioned_state::{
            read_state_version, write_state_version, StateTransformer,
        };
        use templar_common::Nanoseconds;
        use templar_proxy_oracle_governance_kernel::Governance;

        #[test]
        fn v0_to_v1_defaults_self_upgrade_and_migrates_a_pending_admin_upgrade_proposal() {
            testing_env!(VMContextBuilder::new().build());

            let mut old = legacy::State {
                header: Governance {
                    next_id: 8,
                    active_ids: vec![7],
                    ttls: TtlConfigV0 {
                        set_proxy: Nanoseconds::from_secs(1),
                        configure_circuit_breakers: Nanoseconds::from_secs(2),
                        add_circuit_breaker: Nanoseconds::from_secs(3),
                        remove_circuit_breaker: Nanoseconds::from_secs(4),
                        set_manual_trip: Nanoseconds::from_secs(5),
                        rearm: Nanoseconds::from_secs(6),
                        set_enforced: Nanoseconds::from_secs(7),
                        set_action_ttl: Nanoseconds::from_secs(8),
                        set_role: Nanoseconds::from_secs(9),
                        admin_upgrade: Nanoseconds::from_secs(42),
                        admin_function_call: Nanoseconds::from_secs(11),
                    },
                    max_pending_proposals: 64,
                },
                proposals: LookupMap::new(StorageKey::Proposals),
                proxy_oracle_id: "proxy.near".parse().unwrap(),
            };
            // A pending AdminUpgrade proposal stored in the OLD layout (raw `code` blob).
            old.proposals.insert(
                7,
                Proposal {
                    operation: legacy::Operation::AdminUpgrade {
                        code: Base64VecU8(vec![0xde, 0xad, 0xbe, 0xef]),
                        migrate_args: Base64VecU8(vec![0xca, 0xfe]),
                    },
                    created_at: Nanoseconds::from_secs(10),
                    ttl: Nanoseconds::from_secs(15),
                    created_by: "admin.near".parse().unwrap(),
                },
            );
            old.proposals.flush();
            near_sdk::env::state_write(&old);
            write_state_version(0);

            let new = V0ToV1.run().expect("migration succeeds");

            assert_eq!(read_state_version().unwrap(), 1);
            assert_eq!(new.header.next_id, 8);
            assert_eq!(new.header.active_ids, vec![7]);
            assert_eq!(new.header.max_pending_proposals, 64);
            assert_eq!(new.proxy_oracle_id.as_str(), "proxy.near");
            // self_upgrade inherits the admin_upgrade timelock; the other fields carry through.
            assert_eq!(new.header.ttls.self_upgrade, Nanoseconds::from_secs(42));
            assert_eq!(new.header.ttls.admin_upgrade, Nanoseconds::from_secs(42));
            assert_eq!(new.header.ttls.set_proxy, Nanoseconds::from_secs(1));
            assert_eq!(
                new.header.ttls.admin_function_call,
                Nanoseconds::from_secs(11)
            );

            // The pending proposal is now readable in the new layout, its blob reshaped into a
            // `UpgradeSource::Code`, and its metadata preserved.
            let migrated = new.proposals.get(&7).expect("proposal 7 migrated");
            assert_eq!(migrated.created_at, Nanoseconds::from_secs(10));
            assert_eq!(migrated.ttl, Nanoseconds::from_secs(15));
            assert_eq!(migrated.created_by.as_str(), "admin.near");
            match &migrated.operation {
                Operation::AdminUpgrade { code, migrate_args } => {
                    assert_eq!(
                        *code,
                        UpgradeSource::Code(Base64VecU8(vec![0xde, 0xad, 0xbe, 0xef]))
                    );
                    assert_eq!(migrate_args.0, vec![0xca, 0xfe]);
                }
                other => panic!("expected AdminUpgrade, got {other:?}"),
            }
        }

        // `migrate()` classifies no-op / run / reject purely from this framework check — never the
        // caller's args — so a caller can never skip a required migration. The full `migrate()` entry
        // point (a wasm export that installs a panic hook) is exercised end-to-end in the sandbox
        // upgrade tests: the no-op path (same-version oracle refresh), the run path (gov v0→v1), and
        // the revert paths (bad args) — where a bad `migrate` is a failed transaction, not a panic.
        #[test]
        fn needs_migration_is_decided_by_stored_vs_target_version() {
            use templar_common::versioned_state::StateVersion;

            testing_env!(VMContextBuilder::new().build());
            write_state_version(1);
            assert!(!<super::super::State as StateVersion>::needs_migration().unwrap());
            write_state_version(0);
            assert!(<super::super::State as StateVersion>::needs_migration().unwrap());
            write_state_version(2);
            assert!(<super::super::State as StateVersion>::needs_migration().is_err());
        }
    }
}
