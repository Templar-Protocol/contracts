pub mod engine;

use near_sdk::{
    json_types::{Base64VecU8, U128},
    near, AccountId, BorshStorageKey, Gas,
};
use templar_common::oracle::pyth::PriceIdentifier;
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::input::Source;

pub use engine::{error, Event, Governance, Proposal, Validatable};
pub use templar_common::Nanoseconds;

pub const MAX_CIRCUIT_BREAKER_HISTORY_LEN: u32 = 32;
pub const MAX_CIRCUIT_BREAKERS_PER_PROXY: usize = 16;
pub const MAX_PROPOSAL_TTL: Nanoseconds = Nanoseconds::from_secs(180 * 24 * 60 * 60);

macro_rules! governance_operations {
    (
        $(
            $variant:ident => $ttl_field:ident {
                $($field:ident : $field_ty:ty),+ $(,)?
            }
        ),+ $(,)?
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[near(serializers = [json, borsh])]
        pub enum OperationKind {
            $($variant),+
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        #[near(serializers = [json, borsh])]
        pub enum Operation {
            $(
                $variant {
                    $($field: $field_ty),+
                }
            ),+
        }

        impl Operation {
            pub fn kind(&self) -> OperationKind {
                match self {
                    $(Operation::$variant { .. } => OperationKind::$variant),+
                }
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[near(serializers = [json, borsh])]
        pub struct TtlConfig {
            $(pub $ttl_field: Nanoseconds),+
        }

        impl TtlConfig {
            pub fn get(&self, kind: OperationKind) -> Nanoseconds {
                match kind {
                    $(OperationKind::$variant => self.$ttl_field),+
                }
            }

            pub fn set(&mut self, kind: OperationKind, ttl: Nanoseconds) {
                match kind {
                    $(OperationKind::$variant => self.$ttl_field = ttl),+
                }
            }
        }
    };
}

governance_operations! {
    SetProxy => set_proxy {
        id: PriceIdentifier,
        proxy: Option<Proxy<Source>>,
    },
    ConfigureCircuitBreakers => configure_circuit_breakers {
        id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    },
    AddCircuitBreaker => add_circuit_breaker {
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    },
    RemoveCircuitBreaker => remove_circuit_breaker {
        id: PriceIdentifier,
        breaker_id: u32,
    },
    SetManualTrip => set_manual_trip {
        id: PriceIdentifier,
        is_manually_tripped: bool,
        metadata: Option<Vec<u8>>,
    },
    Rearm => rearm {
        id: PriceIdentifier,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: AcceptedHistorySource,
    },
    SetEnforced => set_enforced {
        id: PriceIdentifier,
        breaker_id: u32,
        is_enforced: bool,
    },
    SetActionTtl => set_action_ttl {
        kind: OperationKind,
        new_ttl: Nanoseconds,
    },
    SetRole => set_role {
        account_id: AccountId,
        role: Role,
        set: bool,
    },
    AdminUpgrade => admin_upgrade {
        code: Base64VecU8,
        migrate_args: Base64VecU8,
    },
    AdminFunctionCall => admin_function_call {
        method_name: String,
        args: Base64VecU8,
        attached_deposit: U128,
        gas: Gas,
    },
}

impl Validatable for Operation {
    type OnCreateError = ValidationError;
    type OnExecuteError = ValidationError;

    fn on_create(&self) -> Result<(), Self::OnCreateError> {
        match self {
            Operation::SetProxy {
                proxy: Some(proxy), ..
            } if proxy.sources().len() == 0 => Err(ValidationError::EmptyProxyDefinition),
            Operation::ConfigureCircuitBreakers { config, .. }
                if config.history_len > MAX_CIRCUIT_BREAKER_HISTORY_LEN =>
            {
                Err(ValidationError::CircuitBreakerHistoryTooLong {
                    maximum: MAX_CIRCUIT_BREAKER_HISTORY_LEN,
                    actual: config.history_len,
                })
            }
            Operation::SetActionTtl { new_ttl, .. } if *new_ttl > MAX_PROPOSAL_TTL => {
                Err(ValidationError::TtlExceedsMaximum {
                    maximum: MAX_PROPOSAL_TTL,
                    actual: *new_ttl,
                })
            }
            Operation::AdminUpgrade { code, .. } if code.0.is_empty() => {
                Err(ValidationError::EmptyAdminUpgradeCode)
            }
            Operation::AdminFunctionCall { gas, .. } if gas.is_zero() => {
                Err(ValidationError::ZeroAdminFunctionCallGas)
            }
            Operation::AdminFunctionCall { method_name, .. } if method_name.trim().is_empty() => {
                Err(ValidationError::EmptyAdminFunctionCallMethodName)
            }
            _ => Ok(()),
        }
    }

    fn on_execute(&self) -> Result<(), Self::OnExecuteError> {
        self.on_create()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("Empty proxy definition is not allowed")]
    EmptyProxyDefinition,
    #[error("Circuit breaker history length is too long: maximum {maximum}, got {actual}")]
    CircuitBreakerHistoryTooLong { maximum: u32, actual: u32 },
    #[error("TTL exceeds maximum allowed: maximum {maximum}, got {actual}")]
    TtlExceedsMaximum {
        maximum: Nanoseconds,
        actual: Nanoseconds,
    },
    #[error("Admin upgrade code must not be empty")]
    EmptyAdminUpgradeCode,
    #[error("Admin function call method name must not be empty")]
    EmptyAdminFunctionCallMethodName,
    #[error("Admin function call gas must not be zero")]
    ZeroAdminFunctionCallGas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshStorageKey)]
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

impl Operation {
    pub fn required_role(&self) -> Role {
        match self {
            Operation::SetManualTrip { .. } => Role::ManualTripper,
            Operation::Rearm { .. } | Operation::SetEnforced { .. } => Role::CircuitBreakerOperator,
            Operation::SetProxy { .. }
            | Operation::ConfigureCircuitBreakers { .. }
            | Operation::AddCircuitBreaker { .. }
            | Operation::RemoveCircuitBreaker { .. }
            | Operation::SetActionTtl { .. } => Role::ProxyConfigurationManager,
            Operation::SetRole { .. }
            | Operation::AdminUpgrade { .. }
            | Operation::AdminFunctionCall { .. } => Role::Admin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_proxy_oracle_kernel::proxy::{Aggregator, FreshnessFilter};

    fn invalid_operation() -> Operation {
        Operation::SetProxy {
            id: PriceIdentifier([0xaa; 32]),
            proxy: Some(Proxy::new(
                Aggregator::median_low([]),
                FreshnessFilter::empty(),
            )),
        }
    }

    fn valid_operation() -> Operation {
        Operation::SetProxy {
            id: PriceIdentifier([0xff; 32]),
            proxy: Some(Proxy::new(
                Aggregator::median_low([crate::test_request::pyth(
                    "pyth-oracle.near".parse().unwrap(),
                    PriceIdentifier([0xdd; 32]),
                )
                .into()]),
                FreshnessFilter::empty(),
            )),
        }
    }

    #[rstest::rstest]
    #[case::valid(valid_operation())]
    #[should_panic = "EmptyProxyDefinition"]
    #[case::invalid(invalid_operation())]
    fn on_create(#[case] operation: Operation) {
        operation.on_create().unwrap();
    }

    #[rstest::rstest]
    #[case::valid(valid_operation())]
    #[should_panic = "EmptyProxyDefinition"]
    #[case::invalid(invalid_operation())]
    fn on_execute(#[case] operation: Operation) {
        operation.on_execute().unwrap();
    }

    #[test]
    fn configure_circuit_breakers_rejects_excessive_history_len() {
        let operation = Operation::ConfigureCircuitBreakers {
            id: PriceIdentifier([0xaa; 32]),
            config: CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::zero(),
                history_len: MAX_CIRCUIT_BREAKER_HISTORY_LEN + 1,
            },
        };

        assert_eq!(
            operation.on_create(),
            Err(ValidationError::CircuitBreakerHistoryTooLong {
                maximum: MAX_CIRCUIT_BREAKER_HISTORY_LEN,
                actual: MAX_CIRCUIT_BREAKER_HISTORY_LEN + 1,
            })
        );
        assert_eq!(operation.on_execute(), operation.on_create());
    }

    #[test]
    fn circuit_breaker_operations_require_operator_role() {
        assert_eq!(
            Operation::Rearm {
                id: PriceIdentifier([0xaa; 32]),
                breaker_id: 0,
                armed_after_ns: Nanoseconds::zero(),
                accepted_history_source: AcceptedHistorySource::Empty,
            }
            .required_role(),
            Role::CircuitBreakerOperator
        );

        assert_eq!(
            Operation::SetEnforced {
                id: PriceIdentifier([0xaa; 32]),
                breaker_id: 0,
                is_enforced: false,
            }
            .required_role(),
            Role::CircuitBreakerOperator
        );
    }

    #[test]
    fn proxy_configuration_operations_require_manager_role() {
        let id = PriceIdentifier([0xaa; 32]);
        let config = CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 1,
        };

        assert_eq!(
            Operation::SetProxy { id, proxy: None }.required_role(),
            Role::ProxyConfigurationManager
        );
        assert_eq!(
            Operation::ConfigureCircuitBreakers { id, config }.required_role(),
            Role::ProxyConfigurationManager
        );
        assert_eq!(
            Operation::AddCircuitBreaker {
                id,
                breaker_id: 0,
                breaker: CircuitBreaker::StepwiseChange(
                    templar_proxy_oracle_kernel::proxy::circuit_breaker::StepwiseChange {
                        max_relative_change: "0.1".parse().unwrap(),
                    },
                ),
            }
            .required_role(),
            Role::ProxyConfigurationManager
        );
        assert_eq!(
            Operation::RemoveCircuitBreaker { id, breaker_id: 0 }.required_role(),
            Role::ProxyConfigurationManager
        );
        assert_eq!(
            Operation::SetActionTtl {
                kind: OperationKind::SetProxy,
                new_ttl: Nanoseconds::zero(),
            }
            .required_role(),
            Role::ProxyConfigurationManager
        );
    }

    #[test]
    fn role_json_uses_new_names() {
        assert_eq!(
            near_sdk::serde_json::to_value(Role::ManualTripper).unwrap(),
            near_sdk::serde_json::json!("ManualTripper")
        );
        assert_eq!(
            near_sdk::serde_json::to_value(Role::CircuitBreakerOperator).unwrap(),
            near_sdk::serde_json::json!("CircuitBreakerOperator")
        );
        assert_eq!(
            near_sdk::serde_json::to_value(Role::ProxyConfigurationManager).unwrap(),
            near_sdk::serde_json::json!("ProxyConfigurationManager")
        );
        assert_eq!(
            near_sdk::serde_json::to_value(Role::Admin).unwrap(),
            near_sdk::serde_json::json!("Admin")
        );
    }

    #[test]
    fn operation_kind_mappings_cover_all_operations() {
        let id = PriceIdentifier([0xaa; 32]);
        let breaker = CircuitBreaker::StepwiseChange(
            templar_proxy_oracle_kernel::proxy::circuit_breaker::StepwiseChange {
                max_relative_change: "0.1".parse().unwrap(),
            },
        );
        let config = CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 1,
        };

        let cases = [
            (
                Operation::SetProxy { id, proxy: None },
                OperationKind::SetProxy,
            ),
            (
                Operation::ConfigureCircuitBreakers { id, config },
                OperationKind::ConfigureCircuitBreakers,
            ),
            (
                Operation::AddCircuitBreaker {
                    id,
                    breaker_id: 0,
                    breaker,
                },
                OperationKind::AddCircuitBreaker,
            ),
            (
                Operation::RemoveCircuitBreaker { id, breaker_id: 0 },
                OperationKind::RemoveCircuitBreaker,
            ),
            (
                Operation::SetManualTrip {
                    id,
                    is_manually_tripped: true,
                    metadata: Some(vec![1, 2, 3]),
                },
                OperationKind::SetManualTrip,
            ),
            (
                Operation::Rearm {
                    id,
                    breaker_id: 0,
                    armed_after_ns: Nanoseconds::zero(),
                    accepted_history_source: AcceptedHistorySource::Empty,
                },
                OperationKind::Rearm,
            ),
            (
                Operation::SetEnforced {
                    id,
                    breaker_id: 0,
                    is_enforced: true,
                },
                OperationKind::SetEnforced,
            ),
            (
                Operation::SetActionTtl {
                    kind: OperationKind::SetProxy,
                    new_ttl: Nanoseconds::from_secs(1),
                },
                OperationKind::SetActionTtl,
            ),
            (
                Operation::SetRole {
                    account_id: "operator.near".parse().unwrap(),
                    role: Role::CircuitBreakerOperator,
                    set: true,
                },
                OperationKind::SetRole,
            ),
            (
                Operation::AdminUpgrade {
                    code: Base64VecU8(vec![0xde, 0xad]),
                    migrate_args: Base64VecU8(vec![0xbe, 0xef]),
                },
                OperationKind::AdminUpgrade,
            ),
            (
                Operation::AdminFunctionCall {
                    method_name: "own_accept_owner".to_string(),
                    args: Base64VecU8(b"{}".to_vec()),
                    attached_deposit: U128(0),
                    gas: Gas::from_gas(20_000_000_000_000),
                },
                OperationKind::AdminFunctionCall,
            ),
        ];

        for (operation, expected) in cases {
            assert_eq!(operation.kind(), expected);
        }
    }

    #[test]
    fn ttl_config_get_set_cover_all_operation_kinds() {
        let mut config = TtlConfig {
            set_proxy: Nanoseconds::from_secs(1),
            configure_circuit_breakers: Nanoseconds::from_secs(2),
            add_circuit_breaker: Nanoseconds::from_secs(3),
            remove_circuit_breaker: Nanoseconds::from_secs(4),
            set_manual_trip: Nanoseconds::from_secs(5),
            rearm: Nanoseconds::from_secs(6),
            set_enforced: Nanoseconds::from_secs(7),
            set_action_ttl: Nanoseconds::from_secs(8),
            set_role: Nanoseconds::from_secs(9),
            admin_upgrade: Nanoseconds::from_secs(10),
            admin_function_call: Nanoseconds::from_secs(11),
        };
        let cases = [
            (OperationKind::SetProxy, Nanoseconds::from_secs(1)),
            (
                OperationKind::ConfigureCircuitBreakers,
                Nanoseconds::from_secs(2),
            ),
            (OperationKind::AddCircuitBreaker, Nanoseconds::from_secs(3)),
            (
                OperationKind::RemoveCircuitBreaker,
                Nanoseconds::from_secs(4),
            ),
            (OperationKind::SetManualTrip, Nanoseconds::from_secs(5)),
            (OperationKind::Rearm, Nanoseconds::from_secs(6)),
            (OperationKind::SetEnforced, Nanoseconds::from_secs(7)),
            (OperationKind::SetActionTtl, Nanoseconds::from_secs(8)),
            (OperationKind::SetRole, Nanoseconds::from_secs(9)),
            (OperationKind::AdminUpgrade, Nanoseconds::from_secs(10)),
            (OperationKind::AdminFunctionCall, Nanoseconds::from_secs(11)),
        ];

        for (kind, expected) in cases {
            assert_eq!(config.get(kind), expected);
        }

        for (index, (kind, _)) in cases.into_iter().enumerate() {
            let ttl = Nanoseconds::from_secs(100 + index as u64);
            config.set(kind, ttl);
            assert_eq!(config.get(kind), ttl);
        }
    }

    #[test]
    fn operation_and_ttl_json_shape_stays_named() {
        assert_eq!(
            near_sdk::serde_json::to_value(OperationKind::SetProxy).unwrap(),
            near_sdk::serde_json::json!("SetProxy")
        );

        let config = TtlConfig {
            set_proxy: Nanoseconds::from_secs(1),
            configure_circuit_breakers: Nanoseconds::from_secs(2),
            add_circuit_breaker: Nanoseconds::from_secs(3),
            remove_circuit_breaker: Nanoseconds::from_secs(4),
            set_manual_trip: Nanoseconds::from_secs(5),
            rearm: Nanoseconds::from_secs(6),
            set_enforced: Nanoseconds::from_secs(7),
            set_action_ttl: Nanoseconds::from_secs(8),
            set_role: Nanoseconds::from_secs(9),
            admin_upgrade: Nanoseconds::from_secs(10),
            admin_function_call: Nanoseconds::from_secs(11),
        };
        let config_json = near_sdk::serde_json::to_value(config).unwrap();
        let config_fields = config_json.as_object().unwrap();
        assert!(config_fields.contains_key("set_proxy"));
        assert!(config_fields.contains_key("configure_circuit_breakers"));
        assert!(config_fields.contains_key("add_circuit_breaker"));
        assert!(config_fields.contains_key("remove_circuit_breaker"));
        assert!(config_fields.contains_key("set_manual_trip"));
        assert!(config_fields.contains_key("rearm"));
        assert!(config_fields.contains_key("set_enforced"));
        assert!(config_fields.contains_key("set_action_ttl"));
        assert!(config_fields.contains_key("set_role"));
        assert!(config_fields.contains_key("admin_upgrade"));
        assert!(config_fields.contains_key("admin_function_call"));

        let operation = Operation::SetRole {
            account_id: "operator.near".parse().unwrap(),
            role: Role::CircuitBreakerOperator,
            set: false,
        };
        assert_eq!(
            near_sdk::serde_json::to_value(operation).unwrap(),
            near_sdk::serde_json::json!({
                "SetRole": {
                    "account_id": "operator.near",
                    "role": "CircuitBreakerOperator",
                    "set": false,
                }
            })
        );
    }

    #[test]
    fn set_role_requires_admin_and_has_independent_ttl() {
        let operation = Operation::SetRole {
            account_id: "operator.near".parse().unwrap(),
            role: Role::CircuitBreakerOperator,
            set: true,
        };

        assert_eq!(operation.required_role(), Role::Admin);
        assert_eq!(operation.kind(), OperationKind::SetRole);

        let mut config = TtlConfig {
            set_proxy: Nanoseconds::from_secs(1),
            configure_circuit_breakers: Nanoseconds::from_secs(1),
            add_circuit_breaker: Nanoseconds::from_secs(1),
            remove_circuit_breaker: Nanoseconds::from_secs(1),
            set_manual_trip: Nanoseconds::from_secs(1),
            rearm: Nanoseconds::from_secs(1),
            set_enforced: Nanoseconds::from_secs(1),
            set_action_ttl: Nanoseconds::from_secs(999),
            set_role: Nanoseconds::from_secs(42),
            admin_upgrade: Nanoseconds::from_secs(43),
            admin_function_call: Nanoseconds::from_secs(44),
        };

        assert_eq!(
            config.get(OperationKind::SetRole),
            Nanoseconds::from_secs(42)
        );
        assert_eq!(
            config.get(OperationKind::SetActionTtl),
            Nanoseconds::from_secs(999)
        );

        config.set(OperationKind::SetRole, Nanoseconds::from_secs(100));
        assert_eq!(
            config.get(OperationKind::SetRole),
            Nanoseconds::from_secs(100)
        );
        assert_eq!(
            config.get(OperationKind::SetActionTtl),
            Nanoseconds::from_secs(999)
        );

        config.set(OperationKind::SetActionTtl, Nanoseconds::from_secs(200));
        assert_eq!(
            config.get(OperationKind::SetRole),
            Nanoseconds::from_secs(100)
        );
        assert_eq!(
            config.get(OperationKind::SetActionTtl),
            Nanoseconds::from_secs(200)
        );
    }

    #[test]
    fn admin_upgrade_requires_admin_role() {
        let operation = Operation::AdminUpgrade {
            code: Base64VecU8(vec![0xde, 0xad]),
            migrate_args: Base64VecU8(vec![0xbe, 0xef]),
        };
        assert_eq!(operation.required_role(), Role::Admin);
        assert_eq!(operation.kind(), OperationKind::AdminUpgrade);
    }

    #[test]
    fn admin_upgrade_rejects_empty_code() {
        let operation = Operation::AdminUpgrade {
            code: Base64VecU8(vec![]),
            migrate_args: Base64VecU8(vec![0x00]),
        };
        assert_eq!(
            operation.on_create(),
            Err(ValidationError::EmptyAdminUpgradeCode)
        );
        assert_eq!(operation.on_execute(), operation.on_create());
    }

    #[test]
    fn admin_upgrade_accepts_valid_code() {
        let operation = Operation::AdminUpgrade {
            code: Base64VecU8(vec![0xde, 0xad]),
            migrate_args: Base64VecU8(vec![]),
        };
        assert_eq!(operation.on_create(), Ok(()));
        assert_eq!(operation.on_execute(), Ok(()));
    }

    #[test]
    fn admin_upgrade_ttl_is_independent() {
        let mut config = TtlConfig {
            set_proxy: Nanoseconds::from_secs(1),
            configure_circuit_breakers: Nanoseconds::from_secs(1),
            add_circuit_breaker: Nanoseconds::from_secs(1),
            remove_circuit_breaker: Nanoseconds::from_secs(1),
            set_manual_trip: Nanoseconds::from_secs(1),
            rearm: Nanoseconds::from_secs(1),
            set_enforced: Nanoseconds::from_secs(1),
            set_action_ttl: Nanoseconds::from_secs(1),
            set_role: Nanoseconds::from_secs(1),
            admin_upgrade: Nanoseconds::from_secs(3600),
            admin_function_call: Nanoseconds::from_secs(1),
        };
        assert_eq!(
            config.get(OperationKind::AdminUpgrade),
            Nanoseconds::from_secs(3600)
        );
        config.set(OperationKind::AdminUpgrade, Nanoseconds::from_secs(7200));
        assert_eq!(
            config.get(OperationKind::AdminUpgrade),
            Nanoseconds::from_secs(7200)
        );
    }

    #[test]
    fn admin_upgrade_json_roundtrip() {
        let operation = Operation::AdminUpgrade {
            code: Base64VecU8(vec![0xde, 0xad, 0xbe, 0xef]),
            migrate_args: Base64VecU8(vec![0xca, 0xfe]),
        };
        let json_value = near_sdk::serde_json::to_value(&operation).unwrap();
        // Base64VecU8 serializes as base64 strings in JSON
        assert_eq!(
            json_value,
            near_sdk::serde_json::json!({
                "AdminUpgrade": {
                    "code": "3q2+7w==",
                    "migrate_args": "yv4=",
                }
            })
        );
        let deserialized: Operation = near_sdk::serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, operation);
    }

    #[test]
    fn admin_function_call_requires_admin_role() {
        let operation = Operation::AdminFunctionCall {
            method_name: "own_accept_owner".to_string(),
            args: Base64VecU8(b"{}".to_vec()),
            attached_deposit: U128(0),
            gas: Gas::from_gas(20_000_000_000_000),
        };
        assert_eq!(operation.required_role(), Role::Admin);
        assert_eq!(operation.kind(), OperationKind::AdminFunctionCall);
    }

    #[test]
    fn admin_function_call_json_roundtrip() {
        let operation = Operation::AdminFunctionCall {
            method_name: "own_accept_owner".to_string(),
            args: Base64VecU8(b"{}".to_vec()),
            attached_deposit: U128(1),
            gas: Gas::from_gas(20_000_000_000_000),
        };
        let json_value = near_sdk::serde_json::to_value(&operation).unwrap();
        assert_eq!(
            json_value,
            near_sdk::serde_json::json!({
                "AdminFunctionCall": {
                    "method_name": "own_accept_owner",
                    "args": "e30=",
                    "attached_deposit": "1",
                    "gas": "20000000000000",
                }
            })
        );
        let deserialized: Operation = near_sdk::serde_json::from_value(json_value).unwrap();
        assert_eq!(deserialized, operation);
    }

    #[test]
    fn admin_function_call_rejects_empty_method_name() {
        let operation = Operation::AdminFunctionCall {
            method_name: "   ".to_string(),
            args: Base64VecU8(vec![]),
            attached_deposit: U128(0),
            gas: Gas::from_gas(20_000_000_000_000),
        };
        assert_eq!(
            operation.on_create(),
            Err(ValidationError::EmptyAdminFunctionCallMethodName)
        );
        assert_eq!(operation.on_execute(), operation.on_create());
    }

    #[test]
    fn admin_function_call_rejects_zero_gas() {
        let operation = Operation::AdminFunctionCall {
            method_name: "own_accept_owner".to_string(),
            args: Base64VecU8(vec![]),
            attached_deposit: U128(0),
            gas: Gas::from_gas(0),
        };
        assert_eq!(
            operation.on_create(),
            Err(ValidationError::ZeroAdminFunctionCallGas)
        );
        assert_eq!(operation.on_execute(), operation.on_create());
    }

    #[test]
    fn admin_upgrade_borsh_roundtrip() {
        let operation = Operation::AdminUpgrade {
            code: Base64VecU8(vec![0xde, 0xad, 0xbe, 0xef]),
            migrate_args: Base64VecU8(vec![0xca, 0xfe]),
        };
        let bytes = near_sdk::borsh::to_vec(&operation).unwrap();
        let deserialized: Operation = near_sdk::borsh::from_slice(&bytes).unwrap();
        assert_eq!(deserialized, operation);
    }
}

#[cfg(test)]
mod test_request {
    use near_sdk::AccountId;
    use templar_common::oracle::pyth::PriceIdentifier;
    use templar_proxy_oracle_near_common::request::OracleRequest;

    pub fn pyth(oracle_id: AccountId, price_id: PriceIdentifier) -> OracleRequest {
        OracleRequest::pyth(oracle_id, price_id)
    }
}
