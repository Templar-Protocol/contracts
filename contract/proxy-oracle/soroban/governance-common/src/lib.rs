#![no_std]

use soroban_sdk::{contracterror, contracttype, Address, Bytes, BytesN};
use templar_primitives::Nanoseconds;

use templar_proxy_oracle_soroban_common::{
    is_zero_wasm_hash, Asset, CircuitBreakerConfig, ProxyConfig, RearmConfig, SetEnforcedConfig,
    MAX_MANUAL_TRIP_METADATA_LEN,
};

pub const MAX_PROPOSAL_TTL: Nanoseconds = Nanoseconds::from_secs(180 * 24 * 60 * 60);
pub const MAX_PROPOSAL_TTL_NS: u64 = MAX_PROPOSAL_TTL.as_ns();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum Role {
    Admin,
    ManualTripper,
    CircuitBreakerOperator,
    ProxyConfigurationManager,
}

impl Role {
    pub const ALL: [Self; 4] = [
        Self::Admin,
        Self::ManualTripper,
        Self::CircuitBreakerOperator,
        Self::ProxyConfigurationManager,
    ];
}

#[contracterror]
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    MissingConfig = 3,
    ProposalNotFound = 4,
    ProposalNotMature = 5,
    ArithmeticOverflow = 6,
    RuntimeFailed = 7,
    ProposalOutOfOrder = 8,
    InvalidInput = 9,
    TtlExceedsMaximum = 10,
    LastAdmin = 11,
}

// Generates `GovernanceAction` (the on-chain action union), the parallel
// `OperationKind` tag enum, and the per-kind `TtlConfig` from a single list of
// `Variant(Fields) => ttl_field` entries. A tt-muncher is used rather than a
// flat `$(...)+` so that fieldless variants (e.g. `AcceptOwnership`) are allowed
// alongside tuple variants: each entry is parsed by a dedicated rule (with vs.
// without a `(Fields)` group), and the field group is captured as a whole `tt`
// — re-emitting individual `:ty` fragments into `#[contracttype]` is rejected.
macro_rules! governance_operations {
    // A variant that carries fields: `Variant(F, ...) => ttl_field`.
    (@munch
        evars { $($ev:tt)* }
        pairs { $($pair:tt)* }
        $variant:ident $fields:tt => $ttl_field:ident $(, $($rest:tt)*)?
    ) => {
        governance_operations!(@munch
            evars { $($ev)* $variant $fields, }
            pairs { $($pair)* ($variant $ttl_field) }
            $($($rest)*)?
        );
    };

    // A fieldless variant: `Variant => ttl_field`.
    (@munch
        evars { $($ev:tt)* }
        pairs { $($pair:tt)* }
        $variant:ident => $ttl_field:ident $(, $($rest:tt)*)?
    ) => {
        governance_operations!(@munch
            evars { $($ev)* $variant, }
            pairs { $($pair)* ($variant $ttl_field) }
            $($($rest)*)?
        );
    };

    // All entries consumed — emit the types and impls.
    (@munch
        evars { $($ev:tt)* }
        pairs { $( ($variant:ident $ttl_field:ident) )* }
    ) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[contracttype]
        pub enum OperationKind {
            $($variant),*
        }

        #[derive(Clone, Debug, PartialEq, Eq)]
        #[contracttype]
        pub enum GovernanceAction {
            $($ev)*
        }

        impl GovernanceAction {
            pub fn kind(&self) -> OperationKind {
                match self {
                    $(Self::$variant { .. } => OperationKind::$variant,)*
                }
            }
        }

        #[derive(Clone, Debug, PartialEq, Eq)]
        #[contracttype]
        pub struct TtlConfig {
            $(pub $ttl_field: u64),*
        }

        impl TtlConfig {
            pub fn uniform(ttl: Nanoseconds) -> Self {
                let ttl_ns = ttl.as_ns();
                Self {
                    $($ttl_field: ttl_ns),*
                }
            }

            pub fn get(&self, kind: OperationKind) -> Nanoseconds {
                Nanoseconds::from_ns(self.get_ns(kind))
            }

            pub fn get_ns(&self, kind: OperationKind) -> u64 {
                match kind {
                    $(OperationKind::$variant => self.$ttl_field,)*
                }
            }

            pub fn set(&mut self, kind: OperationKind, ttl: Nanoseconds) {
                self.set_ns(kind, ttl.as_ns());
            }

            pub fn set_ns(&mut self, kind: OperationKind, ttl_ns: u64) {
                match kind {
                    $(OperationKind::$variant => self.$ttl_field = ttl_ns,)*
                }
            }
        }
    };

    // Public entry point: start munching with empty accumulators.
    ( $($input:tt)* ) => {
        governance_operations!(@munch evars {} pairs {} $($input)*);
    };
}

impl templar_proxy_oracle_governance_kernel::TtlConfig<OperationKind> for TtlConfig {
    fn get(&self, kind: OperationKind) -> Nanoseconds {
        self.get(kind)
    }

    fn set(&mut self, kind: OperationKind, ttl: Nanoseconds) {
        self.set(kind, ttl);
    }
}

governance_operations! {
    SetProxy(Asset, ProxyConfig) => set_proxy,
    RemoveProxy(Asset) => remove_proxy,
    ConfigureBreakers(Asset, u64, u32) => configure_breakers,
    AddBreaker(Asset, CircuitBreakerConfig) => add_breaker,
    RemoveBreaker(Asset, u32) => remove_breaker,
    Rearm(Asset, u32, RearmConfig) => rearm,
    SetEnforced(Asset, u32, SetEnforcedConfig) => set_enforced,
    SetManualTrip(Asset, bool, Option<Bytes>) => set_manual_trip,
    TransferOwnership(Address) => transfer_ownership,
    // `AcceptOwnership` and `RenounceOwnership` carry no data — fieldless void
    // cases (CLI: `--operation '"AcceptOwnership"'`).
    AcceptOwnership => accept_ownership,
    RenounceOwnership => renounce_ownership,
    SetActionTtl(OperationKind, u64) => set_action_ttl,
    SetRole(Address, Role, bool) => set_role,
    Upgrade(BytesN<32>) => upgrade,
}

impl GovernanceAction {
    pub fn required_role(&self) -> Role {
        match self {
            Self::SetManualTrip(_, _, _) => Role::ManualTripper,
            Self::Rearm(_, _, _) | Self::SetEnforced(_, _, _) => Role::CircuitBreakerOperator,
            Self::SetProxy(_, _)
            | Self::RemoveProxy(_)
            | Self::ConfigureBreakers(_, _, _)
            | Self::AddBreaker(_, _)
            | Self::RemoveBreaker(_, _)
            | Self::SetActionTtl(_, _) => Role::ProxyConfigurationManager,
            Self::TransferOwnership(_)
            | Self::AcceptOwnership
            | Self::RenounceOwnership
            | Self::SetRole(_, _, _)
            | Self::Upgrade(_) => Role::Admin,
        }
    }

    pub fn action_code(&self) -> u32 {
        match self.kind() {
            OperationKind::SetProxy => 1,
            OperationKind::RemoveProxy => 2,
            OperationKind::ConfigureBreakers => 3,
            OperationKind::AddBreaker => 4,
            OperationKind::RemoveBreaker => 5,
            OperationKind::RenounceOwnership => 6,
            OperationKind::SetManualTrip => 7,
            OperationKind::AcceptOwnership => 8,
            OperationKind::TransferOwnership => 9,
            OperationKind::SetActionTtl => 10,
            OperationKind::SetRole => 11,
            OperationKind::Upgrade => 12,
            OperationKind::Rearm => 13,
            OperationKind::SetEnforced => 14,
        }
    }
}

impl templar_proxy_oracle_governance_kernel::OperationPolicy<TtlConfig> for GovernanceAction {
    type OnCreateError = GovernanceError;
    type OnExecuteError = GovernanceError;

    fn minimum_ttl(&self, ttls: &TtlConfig) -> Nanoseconds {
        match self {
            Self::SetActionTtl(kind, _) => {
                let set_action_ttl = ttls.get(OperationKind::SetActionTtl);
                let target_ttl = ttls.get(*kind);
                set_action_ttl.max(target_ttl)
            }
            _ => ttls.get(self.kind()),
        }
    }

    fn validate_on_create(&self) -> Result<(), Self::OnCreateError> {
        validate_action(self, MAX_MANUAL_TRIP_METADATA_LEN)
    }

    fn validate_on_execute(&self) -> Result<(), Self::OnExecuteError> {
        validate_action(self, MAX_MANUAL_TRIP_METADATA_LEN)
    }
}

pub fn validate_action(
    action: &GovernanceAction,
    max_manual_trip_metadata_len: usize,
) -> Result<(), GovernanceError> {
    match action {
        GovernanceAction::SetProxy(_, config) if config.sources.is_empty() => {
            Err(GovernanceError::InvalidInput)
        }
        GovernanceAction::SetManualTrip(_, _, metadata)
            if metadata
                .as_ref()
                .is_some_and(|metadata| metadata.len() as usize > max_manual_trip_metadata_len) =>
        {
            Err(GovernanceError::InvalidInput)
        }
        GovernanceAction::SetActionTtl(_, new_ttl_ns) if *new_ttl_ns > MAX_PROPOSAL_TTL_NS => {
            Err(GovernanceError::TtlExceedsMaximum)
        }
        GovernanceAction::Upgrade(wasm_hash) if is_zero_wasm_hash(wasm_hash) => {
            Err(GovernanceError::InvalidInput)
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct Proposal {
    pub operation: GovernanceAction,
    pub created_at_ns: u64,
    pub ttl_ns: u64,
    pub created_by: Address,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_lifecycle_ttls_are_independent() {
        let mut config = TtlConfig::uniform(Nanoseconds::from_secs(1));

        config.set(OperationKind::Rearm, Nanoseconds::from_secs(2));
        config.set(OperationKind::SetEnforced, Nanoseconds::from_secs(3));

        assert_eq!(config.get(OperationKind::Rearm), Nanoseconds::from_secs(2));
        assert_eq!(
            config.get(OperationKind::SetEnforced),
            Nanoseconds::from_secs(3)
        );
    }

    #[test]
    fn role_all_lists_every_variant() {
        assert_eq!(Role::ALL.len(), 4);
        assert!(Role::ALL.contains(&Role::Admin));
        assert!(Role::ALL.contains(&Role::ManualTripper));
        assert!(Role::ALL.contains(&Role::CircuitBreakerOperator));
        assert!(Role::ALL.contains(&Role::ProxyConfigurationManager));
    }

    #[test]
    fn action_kind_and_required_role_follow_soroban_policy() {
        let admin_ttl = GovernanceAction::SetActionTtl(OperationKind::Upgrade, 1);
        assert_eq!(admin_ttl.kind(), OperationKind::SetActionTtl);
        assert_eq!(admin_ttl.required_role(), Role::ProxyConfigurationManager);

        let manager_ttl = GovernanceAction::SetActionTtl(OperationKind::SetProxy, 1);
        assert_eq!(manager_ttl.required_role(), Role::ProxyConfigurationManager);

        let manual_trip = GovernanceAction::SetManualTrip(
            Asset::Other(soroban_sdk::Symbol::new(
                &soroban_sdk::Env::default(),
                "BTC",
            )),
            true,
            None,
        );
        assert_eq!(manual_trip.kind(), OperationKind::SetManualTrip);
        assert_eq!(manual_trip.required_role(), Role::ManualTripper);
    }

    #[test]
    fn action_code_covers_all_variants() {
        use templar_proxy_oracle_soroban_common::{
            RearmConfig, SetEnforcedConfig, SorobanDecimal, StepwiseChangeConfig,
        };

        let env = soroban_sdk::Env::default();
        let account = Address::from_string(&soroban_sdk::String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        let asset = Asset::Other(soroban_sdk::Symbol::new(&env, "BTC"));
        let wasm_hash = BytesN::from_array(&env, &[7_u8; 32]);
        let proxy_config = ProxyConfig {
            sources: soroban_sdk::Vec::new(&env),
            min_sources: 1,
            max_age_secs: None,
            max_clock_drift_secs: None,
        };
        let stepwise = CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(
                &env,
                templar_primitives::Decimal::ONE_HALF,
            ),
        });
        let rearm = RearmConfig {
            armed_after_secs: 0,
            accepted_history_source_code: 0,
        };
        let enforced = SetEnforcedConfig { is_enforced: false };

        // The action codes must remain stable.
        let action = GovernanceAction::SetProxy(asset.clone(), proxy_config);
        assert_eq!(action.action_code(), 1);
        let action = GovernanceAction::RemoveProxy(asset.clone());
        assert_eq!(action.action_code(), 2);
        let action = GovernanceAction::ConfigureBreakers(asset.clone(), 0, 8);
        assert_eq!(action.action_code(), 3);
        let action = GovernanceAction::AddBreaker(asset.clone(), stepwise);
        assert_eq!(action.action_code(), 4);
        let action = GovernanceAction::RemoveBreaker(asset.clone(), 0);
        assert_eq!(action.action_code(), 5);
        let action = GovernanceAction::Rearm(asset.clone(), 0, rearm);
        assert_eq!(action.action_code(), 13);
        let action = GovernanceAction::SetEnforced(asset.clone(), 0, enforced);
        assert_eq!(action.action_code(), 14);
        let action = GovernanceAction::SetManualTrip(asset.clone(), true, None);
        assert_eq!(action.action_code(), 7);
        let action = GovernanceAction::RenounceOwnership;
        assert_eq!(action.action_code(), 6);
        let action = GovernanceAction::AcceptOwnership;
        assert_eq!(action.action_code(), 8);
        let action = GovernanceAction::TransferOwnership(account.clone());
        assert_eq!(action.action_code(), 9);
        let action = GovernanceAction::SetActionTtl(OperationKind::SetProxy, 42);
        assert_eq!(action.action_code(), 10);
        let action = GovernanceAction::SetRole(account.clone(), Role::ManualTripper, true);
        assert_eq!(action.action_code(), 11);
        let action = GovernanceAction::Upgrade(wasm_hash);
        assert_eq!(action.action_code(), 12);
    }

    #[test]
    fn validate_action_rejects_empty_proxy_and_large_metadata() {
        let env = soroban_sdk::Env::default();
        let asset = Asset::Other(soroban_sdk::Symbol::new(&env, "BTC"));

        assert_eq!(
            validate_action(
                &GovernanceAction::SetProxy(
                    asset.clone(),
                    ProxyConfig {
                        sources: soroban_sdk::Vec::new(&env),
                        min_sources: 1,
                        max_age_secs: None,
                        max_clock_drift_secs: None,
                    },
                ),
                1024,
            ),
            Err(GovernanceError::InvalidInput)
        );

        assert_eq!(
            validate_action(
                &GovernanceAction::SetManualTrip(
                    asset,
                    true,
                    Some(Bytes::from_array(&env, &[7_u8; 1025])),
                ),
                1024,
            ),
            Err(GovernanceError::InvalidInput)
        );
    }

    #[test]
    fn validate_action_rejects_ttl_exceeding_maximum() {
        assert_eq!(
            validate_action(
                &GovernanceAction::SetActionTtl(OperationKind::SetProxy, MAX_PROPOSAL_TTL_NS + 1),
                1024,
            ),
            Err(GovernanceError::TtlExceedsMaximum)
        );
    }

    #[test]
    fn validate_action_rejects_zero_admin_upgrade_hash() {
        let env = soroban_sdk::Env::default();
        let zero_hash = BytesN::from_array(&env, &[0_u8; 32]);

        assert_eq!(
            validate_action(&GovernanceAction::Upgrade(zero_hash), 1024,),
            Err(GovernanceError::InvalidInput)
        );
    }

    #[test]
    fn is_zero_wasm_hash_detects_all_zeros_and_non_zeros() {
        let env = soroban_sdk::Env::default();
        assert!(is_zero_wasm_hash(&BytesN::from_array(&env, &[0_u8; 32])));
        assert!(!is_zero_wasm_hash(&BytesN::from_array(&env, &[7_u8; 32])));
        let mut partial = [7_u8; 32];
        partial[31] = 0;
        assert!(!is_zero_wasm_hash(&BytesN::from_array(&env, &partial)));
    }
}
