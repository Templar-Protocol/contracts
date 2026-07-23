use super::{legacy, Governance, LookupMap, Operation, Proposal, State, StorageKey};
use near_sdk::near;
use templar_common::{
    panic_with_message,
    versioned_state::{Migrator, StateTransformer},
    UnwrapReject,
};
use templar_proxy_oracle_near_governance_common::LegacyOperation;

/// v0 → v1: seed a `GovernancePolicy` from the old flat TTL table and rewrite every pending proposal
/// body to the new generic form (target ops become `TargetFunctionCall`s dispatching the matching
/// `admin_*` method).
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

        let policy = header.ttls.into_policy();

        // Drain the v0-typed map and drop it so deletions flush before the new-typed map reuses the
        // same storage prefix — otherwise it would read stale v0 bytes.
        let migrated: Vec<(u32, Proposal<Operation>)> = header
            .active_ids
            .iter()
            .filter_map(|&id| {
                let id = u32::try_from(id)
                    .unwrap_or_else(|_| near_sdk::env::panic_str("Proposal ID exceeds u32"));
                let old = old_proposals.remove(&id)?;
                let operation = Operation::try_from(LegacyOperation::from(old.operation))
                    .expect_or_reject("migrate v0 proposal body");
                Some((
                    id,
                    Proposal {
                        operation,
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
                ttls: policy,
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
    fn input_version(&self) -> u32 {
        match self {
            Migration::V0(v0) => v0.input_version(),
        }
    }

    fn output_version(&self) -> u32 {
        match self {
            Migration::V0(v0) => v0.output_version(),
        }
    }

    fn run(self) {
        match self {
            Migration::V0(v0) => {
                v0.run()
                    .unwrap_or_else(|e| panic_with_message(&format!("Failed to migrate V0: {e}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::legacy;
    use super::super::{Operation, Proposal};
    use super::V0ToV1;
    use crate::StorageKey;
    use near_sdk::json_types::Base64VecU8;
    use near_sdk::{store::LookupMap, test_utils::VMContextBuilder, testing_env};
    use templar_common::versioned_state::{
        read_state_version, write_state_version, StateTransformer,
    };
    use templar_common::Nanoseconds;
    use templar_proxy_oracle_governance_kernel::Governance;
    use templar_proxy_oracle_near_governance_common::{
        FunctionCall, LegacyTtlConfig, Role, GAS_FOR_ADMIN_UPGRADE,
    };

    fn legacy_ttls() -> LegacyTtlConfig {
        LegacyTtlConfig {
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
        }
    }

    #[test]
    fn v0_to_v1_seeds_policy_and_migrates_a_pending_admin_upgrade_proposal() {
        testing_env!(VMContextBuilder::new().build());

        let mut old = legacy::State {
            header: Governance {
                next_id: 8,
                active_ids: vec![7],
                ttls: legacy_ttls(),
                max_pending_proposals: 64,
            },
            proposals: LookupMap::new(StorageKey::Proposals),
            proxy_oracle_id: "proxy.near".parse().unwrap(),
        };
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

        // default_target = max target ttl (42s, Admin); reflexive carried over; self_upgrade
        // defaults to admin_upgrade's lock.
        let policy = &new.header.ttls;
        assert_eq!(policy.default_target.ttl, Nanoseconds::from_secs(42));
        assert_eq!(policy.default_target.role, Role::Admin);
        assert_eq!(
            policy.resolve("admin_set_proxy").ttl,
            Nanoseconds::from_secs(1)
        );
        assert_eq!(
            policy.reflexive_ttls.self_upgrade,
            Nanoseconds::from_secs(42)
        );

        // The pending proposal survived the borsh reshape as a generic target call.
        let migrated = new.proposals.get(&7).expect("proposal 7 migrated");
        assert_eq!(migrated.created_at, Nanoseconds::from_secs(10));
        assert_eq!(migrated.ttl, Nanoseconds::from_secs(15));
        assert_eq!(migrated.created_by.as_str(), "admin.near");
        match &migrated.operation {
            Operation::TargetFunctionCall(FunctionCall {
                method_name, gas, ..
            }) => {
                assert_eq!(method_name, "admin_upgrade");
                assert_eq!(*gas, GAS_FOR_ADMIN_UPGRADE);
            }
            reflexive @ Operation::Reflexive(_) => {
                panic!("expected admin_upgrade target call, got {reflexive:?}")
            }
        }
    }

    // The full migrate() run path is exercised in the sandbox upgrade_ordering test; here we cover
    // only the stored-vs-target version decision that drives it.
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
