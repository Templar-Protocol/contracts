pub mod interface;
pub mod legacy;

use std::collections::BTreeMap;

use near_sdk::{
    json_types::{Base64VecU8, U128},
    near, AccountId, BorshStorageKey, Gas,
};
use templar_common::upgrade::UpgradeSource;

pub use interface::{error, Event, Governance, OperationPolicy, Proposal, Validatable};
pub use legacy::{LegacyOperation, LegacyOperationKind, LegacyTtlConfig};
pub use templar_common::Nanoseconds;

/// The longest timelock any proposal may carry (180 days). Bounds both target-method and reflexive
/// TTLs written into [`GovernancePolicy`].
pub const MAX_PROPOSAL_TTL: Nanoseconds = Nanoseconds::from_secs(180 * 24 * 60 * 60);

/// Cap on per-method policy overrides, keeping the single-slot policy blob bounded.
pub const MAX_METHOD_POLICIES: usize = 64;

/// Gas a governance-driven `admin_upgrade` target call needs (a full contract self-deploy + migrate).
pub const GAS_FOR_ADMIN_UPGRADE: Gas = Gas::from_tgas(280);

/// A raw call dispatched to the governed proxy oracle. The generic form every target operation now
/// takes: governance validates only that the method is named and gas is non-zero — semantic
/// validation is the target contract's responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct FunctionCall {
    pub method_name: String,
    pub args: Base64VecU8,
    pub attached_deposit: U128,
    pub gas: Gas,
}

/// The timelock and role required to run a target method. Per-method overrides live in
/// [`GovernancePolicy::method_policies`]; unlisted methods resolve to
/// [`GovernancePolicy::default_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct MethodPolicy {
    pub ttl: Nanoseconds,
    pub role: Role,
}

/// The three reflexive timelock buckets. The policy-editing variants (`SetReflexiveTtl`,
/// `SetTargetDefault`, `SetMethodPolicy`) share the `SetPolicy` bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[near(serializers = [json, borsh])]
pub enum ReflexiveKind {
    SetPolicy,
    SetRole,
    SelfUpgrade,
}

/// Independent per-kind timelocks for reflexive operations. Each field is set independently, so e.g.
/// `self_upgrade` can be strictly longer than `set_role`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct ReflexiveTtls {
    pub set_policy: Nanoseconds,
    pub set_role: Nanoseconds,
    pub self_upgrade: Nanoseconds,
}

impl ReflexiveTtls {
    #[must_use]
    pub fn get(&self, kind: ReflexiveKind) -> Nanoseconds {
        match kind {
            ReflexiveKind::SetPolicy => self.set_policy,
            ReflexiveKind::SetRole => self.set_role,
            ReflexiveKind::SelfUpgrade => self.self_upgrade,
        }
    }

    pub fn set(&mut self, kind: ReflexiveKind, ttl: Nanoseconds) {
        match kind {
            ReflexiveKind::SetPolicy => self.set_policy = ttl,
            ReflexiveKind::SetRole => self.set_role = ttl,
            ReflexiveKind::SelfUpgrade => self.self_upgrade = ttl,
        }
    }
}

/// Operations that mutate governance's own state. `required_role` for these is hardcoded (never
/// resolved through the policy table): policy edits need `ProxyConfigurationManager`, role changes
/// and self-upgrade need `Admin`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum ReflexiveOperation {
    /// Set one reflexive kind's timelock.
    SetReflexiveTtl {
        kind: ReflexiveKind,
        ttl: Nanoseconds,
    },
    /// Set the conservative default policy applied to unlisted target methods.
    SetTargetDefault { policy: MethodPolicy },
    /// Add, update (`Some`), or reset to default (`None`) a per-method policy override.
    SetMethodPolicy {
        method: String,
        policy: Option<MethodPolicy>,
    },
    /// Grant (`set = true`) or revoke (`set = false`) a role for an account.
    SetRole {
        account_id: AccountId,
        role: Role,
        set: bool,
    },
    /// Upgrade the governance contract itself.
    SelfUpgrade {
        code: UpgradeSource,
        migrate_args: Base64VecU8,
    },
}

impl ReflexiveOperation {
    #[must_use]
    pub fn kind(&self) -> ReflexiveKind {
        match self {
            ReflexiveOperation::SetReflexiveTtl { .. }
            | ReflexiveOperation::SetTargetDefault { .. }
            | ReflexiveOperation::SetMethodPolicy { .. } => ReflexiveKind::SetPolicy,
            ReflexiveOperation::SetRole { .. } => ReflexiveKind::SetRole,
            ReflexiveOperation::SelfUpgrade { .. } => ReflexiveKind::SelfUpgrade,
        }
    }
}

/// A governance operation: either self-mutating ([`ReflexiveOperation`]) or a generic call dispatched
/// to the governed proxy oracle ([`FunctionCall`]).
///
/// Deserialization accepts both the current shape and the pre-restructure typed variants
/// (`{"SetProxy":{…}}`, …) via [`compat::OperationWire`]; serialization always emits the current shape.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
#[serde(try_from = "compat::OperationWire")]
pub enum Operation {
    Reflexive(ReflexiveOperation),
    TargetFunctionCall(FunctionCall),
}

mod compat {
    use near_sdk::near;

    use super::{FunctionCall, LegacyOperation, Operation, ReflexiveOperation};

    /// A structural twin of [`Operation`] with the natural (non-`try_from`) derive, so the untagged
    /// wire can recognize the current shape without recursing through `Operation`'s own deserialize.
    #[derive(Debug)]
    #[near(serializers = [json])]
    pub enum OperationNew {
        Reflexive(ReflexiveOperation),
        TargetFunctionCall(FunctionCall),
    }

    /// The current shape is tried first; disjoint variant tags let untagged deserialization fall
    /// through to the legacy shape cleanly.
    #[derive(Debug)]
    #[near(serializers = [json])]
    #[serde(untagged)]
    pub enum OperationWire {
        New(OperationNew),
        Legacy(LegacyOperation),
    }

    impl TryFrom<OperationWire> for Operation {
        type Error = near_sdk::serde_json::Error;

        fn try_from(wire: OperationWire) -> Result<Self, Self::Error> {
            match wire {
                OperationWire::New(OperationNew::Reflexive(reflexive)) => {
                    Ok(Operation::Reflexive(reflexive))
                }
                OperationWire::New(OperationNew::TargetFunctionCall(call)) => {
                    Ok(Operation::TargetFunctionCall(call))
                }
                OperationWire::Legacy(legacy) => Operation::try_from(legacy),
            }
        }
    }
}

/// Coarse operation classification carried on governance events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[near(serializers = [json, borsh])]
pub enum OperationKind {
    SetPolicy,
    SetRole,
    SelfUpgrade,
    TargetFunctionCall,
}

impl Operation {
    #[must_use]
    pub fn kind(&self) -> OperationKind {
        match self {
            Operation::Reflexive(reflexive) => match reflexive.kind() {
                ReflexiveKind::SetPolicy => OperationKind::SetPolicy,
                ReflexiveKind::SetRole => OperationKind::SetRole,
                ReflexiveKind::SelfUpgrade => OperationKind::SelfUpgrade,
            },
            Operation::TargetFunctionCall(_) => OperationKind::TargetFunctionCall,
        }
    }

    /// The target method name, for target calls only. Carried on events so an indexer can see which
    /// method a `TargetFunctionCall` proposal invokes without fetching the full body.
    #[must_use]
    pub fn method(&self) -> Option<String> {
        match self {
            Operation::TargetFunctionCall(call) => Some(call.method_name.clone()),
            Operation::Reflexive(_) => None,
        }
    }

    /// The role a caller must hold to create/execute this operation. Every reflexive op governs
    /// governance itself (the policy table, roles, self-upgrade) and requires `Admin`; target roles
    /// are resolved through the policy table.
    #[must_use]
    pub fn required_role(&self, policy: &GovernancePolicy) -> Role {
        match self {
            Operation::Reflexive(_) => Role::Admin,
            Operation::TargetFunctionCall(call) => policy.resolve(&call.method_name).role,
        }
    }
}

/// The table-driven governance policy: independent reflexive timelocks, a conservative default for
/// target methods, and a whitelist of per-method overrides that may be sped up or given a lower role.
///
/// Invariant: every `method_policies` entry's `ttl` stays `<= default_target.ttl`. Enforced on every
/// write, so an unlisted method (including one introduced by a future target upgrade) can never buy a
/// shorter timelock or lower role than the conservative default.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct GovernancePolicy {
    pub reflexive_ttls: ReflexiveTtls,
    pub default_target: MethodPolicy,
    pub method_policies: BTreeMap<String, MethodPolicy>,
}

impl GovernancePolicy {
    /// The policy governing `method`: its override if listed, otherwise [`Self::default_target`].
    #[must_use]
    pub fn resolve(&self, method: &str) -> MethodPolicy {
        self.method_policies
            .get(method)
            .copied()
            .unwrap_or(self.default_target)
    }

    /// Add/update (`Some`) or reset to default (`None`) a per-method policy.
    ///
    /// # Errors
    ///
    /// If the entry's `ttl` exceeds `default_target.ttl`, or adding a new entry would exceed
    /// [`MAX_METHOD_POLICIES`].
    pub fn set_method_policy(
        &mut self,
        method: String,
        policy: Option<MethodPolicy>,
    ) -> Result<(), PolicyError> {
        match policy {
            Some(policy) => {
                if policy.ttl > self.default_target.ttl {
                    return Err(PolicyError::MethodTtlExceedsDefault {
                        default: self.default_target.ttl,
                        actual: policy.ttl,
                    });
                }
                if !self.method_policies.contains_key(&method)
                    && self.method_policies.len() >= MAX_METHOD_POLICIES
                {
                    return Err(PolicyError::TooManyMethodPolicies {
                        maximum: MAX_METHOD_POLICIES,
                    });
                }
                self.method_policies.insert(method, policy);
            }
            None => {
                self.method_policies.remove(&method);
            }
        }
        Ok(())
    }

    /// Set the conservative default target policy.
    ///
    /// # Errors
    ///
    /// If `policy.ttl` exceeds [`MAX_PROPOSAL_TTL`], or lowering it below an existing method entry
    /// would violate the ceiling invariant.
    pub fn set_target_default(&mut self, policy: MethodPolicy) -> Result<(), PolicyError> {
        if policy.ttl > MAX_PROPOSAL_TTL {
            return Err(PolicyError::TtlExceedsMaximum {
                maximum: MAX_PROPOSAL_TTL,
                actual: policy.ttl,
            });
        }
        if let Some((method, entry)) = self
            .method_policies
            .iter()
            .find(|(_, entry)| entry.ttl > policy.ttl)
        {
            return Err(PolicyError::DefaultBelowExistingMethod {
                default: policy.ttl,
                method: method.clone(),
                method_ttl: entry.ttl,
            });
        }
        self.default_target = policy;
        Ok(())
    }

    /// Set one reflexive kind's timelock.
    ///
    /// # Errors
    ///
    /// If `ttl` exceeds [`MAX_PROPOSAL_TTL`].
    pub fn set_reflexive_ttl(
        &mut self,
        kind: ReflexiveKind,
        ttl: Nanoseconds,
    ) -> Result<(), PolicyError> {
        if ttl > MAX_PROPOSAL_TTL {
            return Err(PolicyError::TtlExceedsMaximum {
                maximum: MAX_PROPOSAL_TTL,
                actual: ttl,
            });
        }
        self.reflexive_ttls.set(kind, ttl);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("Method TTL {actual} exceeds the default target TTL {default}")]
    MethodTtlExceedsDefault {
        default: Nanoseconds,
        actual: Nanoseconds,
    },
    #[error("Default target TTL {default} is below method {method}'s TTL {method_ttl}")]
    DefaultBelowExistingMethod {
        default: Nanoseconds,
        method: String,
        method_ttl: Nanoseconds,
    },
    #[error("Too many method policies: maximum {maximum}")]
    TooManyMethodPolicies { maximum: usize },
    #[error("TTL exceeds maximum allowed: maximum {maximum}, got {actual}")]
    TtlExceedsMaximum {
        maximum: Nanoseconds,
        actual: Nanoseconds,
    },
}

impl Validatable for Operation {
    type OnCreateError = ValidationError;
    type OnExecuteError = ValidationError;

    fn on_create(&self) -> Result<(), Self::OnCreateError> {
        match self {
            Operation::TargetFunctionCall(call) if call.method_name.trim().is_empty() => {
                Err(ValidationError::EmptyFunctionCallMethodName)
            }
            Operation::TargetFunctionCall(call) if call.gas.is_zero() => {
                Err(ValidationError::ZeroFunctionCallGas)
            }
            Operation::Reflexive(ReflexiveOperation::SelfUpgrade { code, .. })
                if code.is_empty_code() =>
            {
                Err(ValidationError::EmptyUpgradeCode)
            }
            Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl { ttl, .. })
                if *ttl > MAX_PROPOSAL_TTL =>
            {
                Err(ValidationError::TtlExceedsMaximum {
                    maximum: MAX_PROPOSAL_TTL,
                    actual: *ttl,
                })
            }
            Operation::Reflexive(ReflexiveOperation::SetTargetDefault { policy })
                if policy.ttl > MAX_PROPOSAL_TTL =>
            {
                Err(ValidationError::TtlExceedsMaximum {
                    maximum: MAX_PROPOSAL_TTL,
                    actual: policy.ttl,
                })
            }
            Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
                policy: Some(policy),
                ..
            }) if policy.ttl > MAX_PROPOSAL_TTL => Err(ValidationError::TtlExceedsMaximum {
                maximum: MAX_PROPOSAL_TTL,
                actual: policy.ttl,
            }),
            _ => Ok(()),
        }
    }

    fn on_execute(&self) -> Result<(), Self::OnExecuteError> {
        self.on_create()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("TTL exceeds maximum allowed: maximum {maximum}, got {actual}")]
    TtlExceedsMaximum {
        maximum: Nanoseconds,
        actual: Nanoseconds,
    },
    #[error("Upgrade code must not be empty")]
    EmptyUpgradeCode,
    #[error("Function call method name must not be empty")]
    EmptyFunctionCallMethodName,
    #[error("Function call gas must not be zero")]
    ZeroFunctionCallGas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshStorageKey)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[near(serializers = [json, borsh])]
pub enum Role {
    ManualTripper,
    CircuitBreakerOperator,
    ProxyConfigurationManager,
    Admin,
}

impl Role {
    pub const ALL: [Self; 4] = [
        Self::ManualTripper,
        Self::CircuitBreakerOperator,
        Self::ProxyConfigurationManager,
        Self::Admin,
    ];
}

impl OperationPolicy<GovernancePolicy> for Operation {
    type OnCreateError = ValidationError;
    type OnExecuteError = ValidationError;

    fn minimum_ttl(&self, policy: &GovernancePolicy) -> Nanoseconds {
        match self {
            // Shortening a reflexive timelock must itself wait at least as long as the bucket being
            // edited, so a lock (e.g. `self_upgrade`) protects its own shortening and stays an
            // effective ceiling.
            Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl { kind, .. }) => policy
                .reflexive_ttls
                .get(ReflexiveKind::SetPolicy)
                .max(policy.reflexive_ttls.get(*kind)),
            Operation::Reflexive(reflexive) => policy.reflexive_ttls.get(reflexive.kind()),
            Operation::TargetFunctionCall(call) => policy.resolve(&call.method_name).ttl,
        }
    }

    fn validate_on_create(&self) -> Result<(), Self::OnCreateError> {
        <Self as Validatable>::on_create(self)
    }

    fn validate_on_execute(&self) -> Result<(), Self::OnExecuteError> {
        <Self as Validatable>::on_execute(self)
    }
}

#[cfg(test)]
mod tests;
