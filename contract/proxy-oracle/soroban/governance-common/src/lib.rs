#![no_std]

use soroban_sdk::{contracterror, contracttype, Address, Bytes, BytesN};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::is_valid_relative_change;

use templar_proxy_oracle_soroban_common::{
    is_zero_wasm_hash, parse_breaker_set_config, parse_proxy_config, Asset, CircuitBreakerConfig,
    ProxyConfig, RearmConfig, SetEnforcedConfig, MAX_MANUAL_TRIP_METADATA_LEN,
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
    TooManyPendingProposals = 12,
    PendingProposalBytesExceeded = 13,
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
            CancelOwnershipTransfer(Address),
        }

        impl GovernanceAction {
            pub fn kind(&self) -> OperationKind {
                match self {
                    $(Self::$variant { .. } => OperationKind::$variant,)*
                    Self::CancelOwnershipTransfer(_) => OperationKind::TransferOwnership,
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

impl OperationKind {
    pub const fn required_role(self) -> Role {
        match self {
            Self::SetManualTrip => Role::ManualTripper,
            Self::Rearm | Self::SetEnforced => Role::CircuitBreakerOperator,
            Self::SetProxy
            | Self::RemoveProxy
            | Self::ConfigureBreakers
            | Self::AddBreaker
            | Self::RemoveBreaker => Role::ProxyConfigurationManager,
            Self::TransferOwnership
            | Self::AcceptOwnership
            | Self::RenounceOwnership
            | Self::SetActionTtl
            | Self::SetRole
            | Self::Upgrade => Role::Admin,
        }
    }
}

impl GovernanceAction {
    pub fn required_role(&self) -> Role {
        match self {
            Self::SetActionTtl(target, _) if target.required_role() == Role::Admin => Role::Admin,
            Self::SetActionTtl(_, _) => Role::ProxyConfigurationManager,
            _ => self.kind().required_role(),
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
        GovernanceAction::SetProxy(_, config) => parse_proxy_config(config)
            .map(|_| ())
            .map_err(|_| GovernanceError::InvalidInput),
        GovernanceAction::ConfigureBreakers(_, sample_interval_secs, history_len) => {
            parse_breaker_set_config(*sample_interval_secs, *history_len)
                .map(|_| ())
                .map_err(|_| GovernanceError::InvalidInput)
        }
        GovernanceAction::AddBreaker(_, CircuitBreakerConfig::CumulativeChange(config)) => {
            if is_valid_relative_change(config.max_relative_change.to_decimal()) {
                Ok(())
            } else {
                Err(GovernanceError::InvalidInput)
            }
        }
        GovernanceAction::AddBreaker(_, config) => config
            .to_kernel(None)
            .map(|_| ())
            .map_err(|_| GovernanceError::InvalidInput),
        GovernanceAction::Rearm(_, _, config) => {
            Nanoseconds::checked_from_secs(config.arming_delay_secs)
                .map(|_| ())
                .ok_or(GovernanceError::InvalidInput)
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
    fn set_proxy_static_admission_accepts_all_methods_and_rejects_bad_pins() {
        use soroban_sdk::testutils::Address as _;
        use templar_proxy_oracle_soroban_common::{AggregatorConfig, MedianConfig, SourceConfig};

        let env = soroban_sdk::Env::default();
        let asset = Asset::Other(soroban_sdk::Symbol::new(&env, "BTC"));
        for decimals in [8, 19] {
            let sources = soroban_sdk::vec![
                &env,
                SourceConfig {
                    oracle: Address::generate(&env),
                    asset: asset.clone(),
                    expected_decimals: decimals,
                }
            ];
            for aggregator in [
                AggregatorConfig::MedianLow(MedianConfig {
                    sources: sources.clone(),
                    min_sources: 1,
                }),
                AggregatorConfig::MedianHigh(MedianConfig {
                    sources: sources.clone(),
                    min_sources: 1,
                }),
                AggregatorConfig::Priority(sources),
            ] {
                let action = GovernanceAction::SetProxy(
                    asset.clone(),
                    ProxyConfig {
                        aggregator,
                        max_age_secs: 0,
                        max_clock_drift_secs: 0,
                    },
                );
                assert_eq!(validate_action(&action, 1024).is_ok(), decimals <= 18);
            }
        }
    }

    #[test]
    fn static_breaker_admission_checks_feasibility_without_runtime_state() {
        use templar_primitives::Decimal;
        use templar_proxy_oracle_soroban_common::{
            CumulativeChangeConfig, MonotonicRunConfig, SorobanDecimal, StepwiseChangeConfig,
            WindowedChangeDeltaConfig, MAX_HISTORY_RECORDS,
        };

        let env = soroban_sdk::Env::default();
        let asset = Asset::Other(soroban_sdk::Symbol::new(&env, "BTC"));
        for threshold in [Decimal::ZERO, Decimal::ONE, Decimal::TWO] {
            let threshold_config = SorobanDecimal::from_decimal(&env, threshold);
            for config in [
                CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                    max_relative_change: threshold_config.clone(),
                }),
                CircuitBreakerConfig::MonotonicRun(MonotonicRunConfig {
                    max_streak: MAX_HISTORY_RECORDS - 1,
                    min_relative_step_change: threshold_config.clone(),
                }),
                CircuitBreakerConfig::WindowedChangeDelta(WindowedChangeDeltaConfig {
                    window_len: 2,
                    lookback_windows: 1,
                    max_relative_mean_change: threshold_config.clone(),
                }),
                CircuitBreakerConfig::CumulativeChange(CumulativeChangeConfig {
                    max_relative_change: threshold_config,
                }),
            ] {
                assert_eq!(
                    validate_action(&GovernanceAction::AddBreaker(asset.clone(), config), 1024)
                        .is_ok(),
                    threshold == Decimal::ONE
                );
            }
        }
        let threshold = SorobanDecimal::from_decimal(&env, Decimal::ONE);
        for config in [
            CircuitBreakerConfig::MonotonicRun(MonotonicRunConfig {
                max_streak: MAX_HISTORY_RECORDS,
                min_relative_step_change: threshold.clone(),
            }),
            CircuitBreakerConfig::WindowedChangeDelta(WindowedChangeDeltaConfig {
                window_len: 17,
                lookback_windows: 1,
                max_relative_mean_change: threshold,
            }),
        ] {
            assert_eq!(
                validate_action(&GovernanceAction::AddBreaker(asset.clone(), config), 1024),
                Err(GovernanceError::InvalidInput)
            );
        }
    }

    #[test]
    fn static_interval_admission_rejects_overflow_but_retains_zero() {
        use templar_proxy_oracle_soroban_common::MAX_HISTORY_RECORDS;

        let env = soroban_sdk::Env::default();
        let asset = Asset::Other(soroban_sdk::Symbol::new(&env, "BTC"));
        let max_secs = u64::MAX / 1_000_000_000;
        for seconds in [0, max_secs, max_secs + 1] {
            let expected = if seconds <= max_secs {
                Ok(())
            } else {
                Err(GovernanceError::InvalidInput)
            };
            assert_eq!(
                validate_action(
                    &GovernanceAction::ConfigureBreakers(
                        asset.clone(),
                        seconds,
                        MAX_HISTORY_RECORDS,
                    ),
                    1024,
                ),
                expected
            );
            assert_eq!(
                validate_action(
                    &GovernanceAction::Rearm(
                        asset.clone(),
                        0,
                        RearmConfig {
                            arming_delay_secs: seconds,
                        },
                    ),
                    1024,
                ),
                expected
            );
        }
        for history_len in [0, MAX_HISTORY_RECORDS + 1] {
            assert_eq!(
                validate_action(
                    &GovernanceAction::ConfigureBreakers(asset.clone(), 0, history_len),
                    1024,
                ),
                Err(GovernanceError::InvalidInput)
            );
        }
    }

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
    fn operation_kind_required_role_is_exhaustive() {
        for (kind, role) in [
            (OperationKind::SetProxy, Role::ProxyConfigurationManager),
            (OperationKind::RemoveProxy, Role::ProxyConfigurationManager),
            (
                OperationKind::ConfigureBreakers,
                Role::ProxyConfigurationManager,
            ),
            (OperationKind::AddBreaker, Role::ProxyConfigurationManager),
            (
                OperationKind::RemoveBreaker,
                Role::ProxyConfigurationManager,
            ),
            (OperationKind::Rearm, Role::CircuitBreakerOperator),
            (OperationKind::SetEnforced, Role::CircuitBreakerOperator),
            (OperationKind::SetManualTrip, Role::ManualTripper),
            (OperationKind::TransferOwnership, Role::Admin),
            (OperationKind::AcceptOwnership, Role::Admin),
            (OperationKind::RenounceOwnership, Role::Admin),
            (OperationKind::SetActionTtl, Role::Admin),
            (OperationKind::SetRole, Role::Admin),
            (OperationKind::Upgrade, Role::Admin),
        ] {
            assert_eq!(kind.required_role(), role);
        }

        let admin_ttl = GovernanceAction::SetActionTtl(OperationKind::Upgrade, 1);
        assert_eq!(admin_ttl.required_role(), Role::Admin);

        let manager_ttl = GovernanceAction::SetActionTtl(OperationKind::SetProxy, 1);
        assert_eq!(manager_ttl.required_role(), Role::ProxyConfigurationManager);
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
            aggregator: templar_proxy_oracle_soroban_common::AggregatorConfig::Priority(
                soroban_sdk::Vec::new(&env),
            ),
            max_age_secs: 30,
            max_clock_drift_secs: 5,
        };
        let stepwise = CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
            max_relative_change: SorobanDecimal::from_decimal(
                &env,
                templar_primitives::Decimal::ONE_HALF,
            ),
        });
        let rearm = RearmConfig {
            arming_delay_secs: 0,
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
                        aggregator: templar_proxy_oracle_soroban_common::AggregatorConfig::Priority(
                            soroban_sdk::Vec::new(&env),
                        ),
                        max_age_secs: 30,
                        max_clock_drift_secs: 5,
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
    fn set_proxy_action_uses_shared_proxy_config_validation() {
        use soroban_sdk::testutils::Address as _;

        let env = soroban_sdk::Env::default();
        let asset = Asset::Other(soroban_sdk::Symbol::new(&env, "BTC"));
        let mut sources = soroban_sdk::Vec::new(&env);
        for _ in 0..3 {
            sources.push_back(templar_proxy_oracle_soroban_common::SourceConfig {
                oracle: Address::generate(&env),
                asset: asset.clone(),
                expected_decimals: 8,
            });
        }
        let mut config = ProxyConfig {
            aggregator: templar_proxy_oracle_soroban_common::AggregatorConfig::MedianLow(
                templar_proxy_oracle_soroban_common::MedianConfig {
                    sources,
                    min_sources: 3,
                },
            ),
            max_age_secs: 30,
            max_clock_drift_secs: 5,
        };
        assert_eq!(
            validate_action(
                &GovernanceAction::SetProxy(asset.clone(), config.clone()),
                1024,
            ),
            Ok(())
        );

        config.max_age_secs = templar_proxy_oracle_soroban_common::MAX_SOURCE_AGE_SECS + 1;
        assert_eq!(
            validate_action(&GovernanceAction::SetProxy(asset, config), 1024),
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

    #[test]
    fn ownership_cancellation_reuses_existing_policy_and_error_numbers_are_stable() {
        use soroban_sdk::{testutils::Address as _, Env};
        use templar_proxy_oracle_governance_kernel::OperationPolicy;
        let env = Env::default();
        let action = GovernanceAction::CancelOwnershipTransfer(Address::generate(&env));
        let mut ttls = TtlConfig::uniform(Nanoseconds::zero());
        ttls.set(
            OperationKind::TransferOwnership,
            Nanoseconds::from_secs(123),
        );
        assert_eq!(action.kind(), OperationKind::TransferOwnership);
        assert_eq!(action.required_role(), Role::Admin);
        assert_eq!(action.action_code(), 9);
        assert_eq!(action.minimum_ttl(&ttls), Nanoseconds::from_secs(123));
        assert_eq!(GovernanceError::AlreadyInitialized as u32, 1);
        assert_eq!(GovernanceError::RuntimeFailed as u32, 7);
        assert_eq!(GovernanceError::ProposalOutOfOrder as u32, 8);
        assert_eq!(GovernanceError::InvalidInput as u32, 9);
        assert_eq!(GovernanceError::LastAdmin as u32, 11);
        assert_eq!(GovernanceError::TooManyPendingProposals as u32, 12);
        assert_eq!(GovernanceError::PendingProposalBytesExceeded as u32, 13);
    }
}
