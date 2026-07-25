//! The pre-restructure operation set (12 typed variants) and its mapping to the new generic
//! [`Operation`]. Consumed by the borsh state migration and legacy-JSON acceptance on
//! `create_proposal`; the manager CLI builds current-format operations directly via [`crate::target`].
//! Target-call serialization is shared with the CLI through that module.
//!
//! The old create-time payload checks (`EmptyProxyDefinition`, `CircuitBreakerHistoryTooLong`) are not
//! reproduced here — the generic form is opaque to governance, so they move to the proxy oracle
//! (ENG-520).

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

use crate::{
    target, FunctionCall, GovernancePolicy, MethodPolicy, Operation, ReflexiveKind,
    ReflexiveOperation, Role,
};

/// The old per-operation kind tag, retained so a legacy `SetActionTtl { kind, .. }` still resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum LegacyOperationKind {
    SetProxy,
    ConfigureCircuitBreakers,
    AddCircuitBreaker,
    RemoveCircuitBreaker,
    SetManualTrip,
    Rearm,
    SetEnforced,
    SetActionTtl,
    SetRole,
    AdminUpgrade,
    AdminFunctionCall,
    SelfUpgrade,
}

/// The pre-restructure `Operation`: one typed variant per governance action.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum LegacyOperation {
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
        kind: LegacyOperationKind,
        new_ttl: Nanoseconds,
    },
    SetRole {
        account_id: AccountId,
        role: Role,
        set: bool,
    },
    AdminUpgrade {
        code: UpgradeSource,
        migrate_args: Base64VecU8,
    },
    AdminFunctionCall {
        method_name: String,
        args: Base64VecU8,
        attached_deposit: U128,
        gas: Gas,
    },
    SelfUpgrade {
        code: UpgradeSource,
        migrate_args: Base64VecU8,
    },
}

/// The 11-field pre-restructure TTL table (also the shape of the on-chain v0 config, seen by the
/// migration). Retained only to seed a [`GovernancePolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct LegacyTtlConfig {
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

impl LegacyTtlConfig {
    /// Seed a [`GovernancePolicy`] from the old flat TTL table.
    ///
    /// * `default_target` = the maximum of every old target-op TTL (conservative ceiling), role
    ///   `Admin`.
    /// * `method_policies` seed each `admin_*` method with its old TTL and natural role.
    /// * `reflexive_ttls` carry over from the reflexive fields; `self_upgrade` defaults to
    ///   `admin_upgrade` (the v0 table had no independent self-upgrade lock).
    #[must_use]
    pub fn into_policy(self) -> GovernancePolicy {
        let default_ttl = [
            self.set_proxy,
            self.configure_circuit_breakers,
            self.add_circuit_breaker,
            self.remove_circuit_breaker,
            self.set_manual_trip,
            self.rearm,
            self.set_enforced,
            self.admin_upgrade,
            self.admin_function_call,
        ]
        .into_iter()
        .max()
        .unwrap_or_else(Nanoseconds::zero);

        let method_policies = [
            (
                "admin_set_proxy",
                self.set_proxy,
                Role::ProxyConfigurationManager,
            ),
            (
                "admin_configure_circuit_breakers",
                self.configure_circuit_breakers,
                Role::ProxyConfigurationManager,
            ),
            (
                "admin_add_circuit_breaker",
                self.add_circuit_breaker,
                Role::ProxyConfigurationManager,
            ),
            (
                "admin_remove_circuit_breaker",
                self.remove_circuit_breaker,
                Role::ProxyConfigurationManager,
            ),
            (
                "admin_set_manual_trip",
                self.set_manual_trip,
                Role::ManualTripper,
            ),
            ("admin_rearm", self.rearm, Role::CircuitBreakerOperator),
            (
                "admin_set_enforced",
                self.set_enforced,
                Role::CircuitBreakerOperator,
            ),
            ("admin_upgrade", self.admin_upgrade, Role::Admin),
        ]
        .into_iter()
        .map(|(method, ttl, role)| (method.to_owned(), MethodPolicy { ttl, role }))
        .collect();

        GovernancePolicy {
            reflexive_ttls: crate::ReflexiveTtls {
                set_policy: self.set_action_ttl,
                set_role: self.set_role,
                self_upgrade: self.admin_upgrade,
            },
            default_target: MethodPolicy {
                ttl: default_ttl,
                role: Role::Admin,
            },
            method_policies,
        }
    }
}

/// Map a legacy per-kind TTL edit to the equivalent new reflexive policy edit.
fn map_set_action_ttl(kind: LegacyOperationKind, ttl: Nanoseconds) -> Operation {
    use LegacyOperationKind as K;
    let method_edit = |method: &str, role: Role| {
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: method.to_owned(),
            policy: Some(MethodPolicy { ttl, role }),
        })
    };
    let reflexive = |kind: ReflexiveKind| {
        Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl { kind, ttl })
    };
    match kind {
        K::SetProxy => method_edit("admin_set_proxy", Role::ProxyConfigurationManager),
        K::ConfigureCircuitBreakers => method_edit(
            "admin_configure_circuit_breakers",
            Role::ProxyConfigurationManager,
        ),
        K::AddCircuitBreaker => {
            method_edit("admin_add_circuit_breaker", Role::ProxyConfigurationManager)
        }
        K::RemoveCircuitBreaker => method_edit(
            "admin_remove_circuit_breaker",
            Role::ProxyConfigurationManager,
        ),
        K::SetManualTrip => method_edit("admin_set_manual_trip", Role::ManualTripper),
        K::Rearm => method_edit("admin_rearm", Role::CircuitBreakerOperator),
        K::SetEnforced => method_edit("admin_set_enforced", Role::CircuitBreakerOperator),
        K::AdminUpgrade => method_edit("admin_upgrade", Role::Admin),
        K::AdminFunctionCall => Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
            policy: MethodPolicy {
                ttl,
                role: Role::Admin,
            },
        }),
        K::SetActionTtl => reflexive(ReflexiveKind::SetPolicy),
        K::SetRole => reflexive(ReflexiveKind::SetRole),
        K::SelfUpgrade => reflexive(ReflexiveKind::SelfUpgrade),
    }
}

impl LegacyOperation {
    /// Map to the generic [`Operation`], attaching `gas_override` to the dispatched target call when
    /// given (otherwise the method's default). The migration and legacy-JSON paths pass `None`; the CLI
    /// passes an operator-supplied override for the target subcommands that expose `--gas`.
    ///
    /// # Errors
    ///
    /// If serializing a target method's args to JSON fails.
    pub fn into_operation(
        self,
        gas_override: Option<Gas>,
    ) -> Result<Operation, near_sdk::serde_json::Error> {
        Ok(match self {
            LegacyOperation::SetProxy { id, proxy } => {
                Operation::TargetFunctionCall(target::admin_set_proxy(id, proxy, gas_override)?)
            }
            LegacyOperation::ConfigureCircuitBreakers { id, config } => {
                Operation::TargetFunctionCall(target::admin_configure_circuit_breakers(
                    id,
                    config,
                    gas_override,
                )?)
            }
            LegacyOperation::AddCircuitBreaker {
                id,
                breaker_id,
                breaker,
            } => Operation::TargetFunctionCall(target::admin_add_circuit_breaker(
                id,
                breaker_id,
                breaker,
                gas_override,
            )?),
            LegacyOperation::RemoveCircuitBreaker { id, breaker_id } => {
                Operation::TargetFunctionCall(target::admin_remove_circuit_breaker(
                    id,
                    breaker_id,
                    gas_override,
                )?)
            }
            LegacyOperation::SetManualTrip {
                id,
                is_manually_tripped,
                metadata,
            } => Operation::TargetFunctionCall(target::admin_set_manual_trip(
                id,
                is_manually_tripped,
                metadata,
                gas_override,
            )?),
            LegacyOperation::Rearm {
                id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            } => Operation::TargetFunctionCall(target::admin_rearm(
                id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
                gas_override,
            )?),
            LegacyOperation::SetEnforced {
                id,
                breaker_id,
                is_enforced,
            } => Operation::TargetFunctionCall(target::admin_set_enforced(
                id,
                breaker_id,
                is_enforced,
                gas_override,
            )?),
            LegacyOperation::AdminUpgrade { code, migrate_args } => Operation::TargetFunctionCall(
                target::admin_upgrade(code, migrate_args, gas_override)?,
            ),
            LegacyOperation::AdminFunctionCall {
                method_name,
                args,
                attached_deposit,
                gas,
            } => Operation::TargetFunctionCall(FunctionCall {
                method_name,
                args,
                attached_deposit,
                gas,
            }),
            LegacyOperation::SetActionTtl { kind, new_ttl } => map_set_action_ttl(kind, new_ttl),
            LegacyOperation::SetRole {
                account_id,
                role,
                set,
            } => Operation::Reflexive(ReflexiveOperation::SetRole {
                account_id,
                role,
                set,
            }),
            LegacyOperation::SelfUpgrade { code, migrate_args } => {
                Operation::Reflexive(ReflexiveOperation::SelfUpgrade { code, migrate_args })
            }
        })
    }
}

impl TryFrom<LegacyOperation> for Operation {
    type Error = near_sdk::serde_json::Error;

    /// Maps with each target method's default gas. Callers that want an override use
    /// [`LegacyOperation::into_operation`].
    fn try_from(operation: LegacyOperation) -> Result<Self, Self::Error> {
        operation.into_operation(None)
    }
}
