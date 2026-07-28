use super::*;
use near_sdk::json_types::Base58CryptoHash;

fn method_policy(ttl_secs: u64, role: Role) -> MethodPolicy {
    MethodPolicy {
        ttl: Nanoseconds::from_secs(ttl_secs),
        role,
    }
}

fn policy(default_ttl_secs: u64) -> GovernancePolicy {
    GovernancePolicy {
        reflexive_ttls: ReflexiveTtls {
            set_policy: Nanoseconds::zero(),
            set_role: Nanoseconds::zero(),
            self_upgrade: Nanoseconds::zero(),
        },
        default_target: method_policy(default_ttl_secs, Role::Admin),
        method_policies: BTreeMap::new(),
    }
}

fn target(method: &str, gas_tgas: u64) -> Operation {
    Operation::TargetFunctionCall(FunctionCall {
        method_name: method.to_owned(),
        args: Base64VecU8(b"{}".to_vec()),
        attached_deposit: U128(0),
        gas: Gas::from_tgas(gas_tgas),
    })
}

#[test]
fn resolve_falls_back_to_default_for_unlisted_method() {
    let mut policy = policy(100);
    policy
        .set_method_policy(
            "admin_set_manual_trip".to_owned(),
            Some(method_policy(5, Role::ManualTripper)),
        )
        .unwrap();

    let listed = policy.resolve("admin_set_manual_trip");
    assert_eq!(listed.ttl, Nanoseconds::from_secs(5));
    assert_eq!(listed.role, Role::ManualTripper);

    let unlisted = policy.resolve("admin_anything_new");
    assert_eq!(unlisted.ttl, Nanoseconds::from_secs(100));
    assert_eq!(unlisted.role, Role::Admin);
}

#[test]
fn set_method_policy_rejects_ttl_above_default() {
    let mut policy = policy(10);
    assert_eq!(
        policy.set_method_policy(
            "admin_set_proxy".to_owned(),
            Some(method_policy(11, Role::ProxyConfigurationManager)),
        ),
        Err(PolicyError::MethodTtlExceedsDefault {
            default: Nanoseconds::from_secs(10),
            actual: Nanoseconds::from_secs(11),
        })
    );
    assert!(policy.method_policies.is_empty());
}

#[test]
fn set_target_default_cannot_drop_below_an_existing_entry() {
    let mut policy = policy(100);
    policy
        .set_method_policy(
            "admin_set_proxy".to_owned(),
            Some(method_policy(50, Role::ProxyConfigurationManager)),
        )
        .unwrap();

    assert!(matches!(
        policy.set_target_default(method_policy(40, Role::Admin)),
        Err(PolicyError::DefaultBelowExistingMethod { .. })
    ));
    // Lowering to exactly the entry's ttl is allowed.
    policy
        .set_target_default(method_policy(50, Role::Admin))
        .unwrap();
}

#[test]
fn set_target_default_bounded_by_max_proposal_ttl() {
    let mut policy = policy(1);
    let over = MethodPolicy {
        ttl: MAX_PROPOSAL_TTL.saturating_add(Nanoseconds::from_secs(1)),
        role: Role::Admin,
    };
    assert!(matches!(
        policy.set_target_default(over),
        Err(PolicyError::TtlExceedsMaximum { .. })
    ));
}

fn policy_wire(default_ttl_secs: u64) -> GovernancePolicyWire {
    GovernancePolicyWire {
        reflexive_ttls: ReflexiveTtls {
            set_policy: Nanoseconds::zero(),
            set_role: Nanoseconds::zero(),
            self_upgrade: Nanoseconds::zero(),
        },
        default_target: method_policy(default_ttl_secs, Role::Admin),
        method_policies: BTreeMap::new(),
    }
}

#[test]
fn policy_wire_parses_when_it_satisfies_the_invariants() {
    let mut wire = policy_wire(100);
    wire.reflexive_ttls.set_policy = Nanoseconds::from_secs(10);
    wire.method_policies.insert(
        "admin_set_manual_trip".to_owned(),
        method_policy(5, Role::ManualTripper),
    );

    let parsed = GovernancePolicy::try_from(wire).unwrap();
    assert_eq!(
        parsed.resolve("admin_set_manual_trip").ttl,
        Nanoseconds::from_secs(5)
    );
    assert_eq!(parsed.reflexive_ttls.set_policy, Nanoseconds::from_secs(10));
}

#[test]
fn policy_wire_rejects_every_invariant_init_would_otherwise_bypass() {
    let over_max = MAX_PROPOSAL_TTL.saturating_add(Nanoseconds::from_secs(1));

    // Every reflexive bucket is bounded, not just the first: an oversized `set_policy` is the
    // bricking case (no policy-repair proposal can then be created).
    for kind in [
        ReflexiveKind::SetPolicy,
        ReflexiveKind::SetRole,
        ReflexiveKind::SelfUpgrade,
    ] {
        let mut wire = policy_wire(1);
        wire.reflexive_ttls.set(kind, over_max);
        assert_eq!(
            GovernancePolicy::try_from(wire),
            Err(PolicyError::TtlExceedsMaximum {
                maximum: MAX_PROPOSAL_TTL,
                actual: over_max,
            }),
            "{kind:?} bucket not bounded"
        );
    }

    let mut default_over = policy_wire(1);
    default_over.default_target.ttl = over_max;
    assert!(matches!(
        GovernancePolicy::try_from(default_over),
        Err(PolicyError::TtlExceedsMaximum { .. })
    ));

    let mut ceiling = policy_wire(10);
    ceiling.method_policies.insert(
        "admin_set_proxy".to_owned(),
        method_policy(11, Role::ProxyConfigurationManager),
    );
    assert_eq!(
        GovernancePolicy::try_from(ceiling),
        Err(PolicyError::MethodTtlExceedsDefault {
            default: Nanoseconds::from_secs(10),
            actual: Nanoseconds::from_secs(11),
        })
    );

    let mut too_many = policy_wire(1_000);
    for index in 0..=MAX_METHOD_POLICIES {
        too_many
            .method_policies
            .insert(format!("admin_m{index}"), method_policy(1, Role::Admin));
    }
    assert_eq!(
        GovernancePolicy::try_from(too_many),
        Err(PolicyError::TooManyMethodPolicies {
            maximum: MAX_METHOD_POLICIES,
        })
    );
}

/// The bound is on the wire, not on a helper someone has to remember to call: an out-of-range policy
/// never deserializes into a `GovernancePolicy` at all, which is what init args go through.
#[test]
fn out_of_range_policy_json_does_not_deserialize() {
    let over_max = MAX_PROPOSAL_TTL.saturating_add(Nanoseconds::from_secs(1));
    let mut wire = policy_wire(1);
    wire.reflexive_ttls.set_policy = over_max;
    let json = near_sdk::serde_json::to_string(&wire).unwrap();

    let error = near_sdk::serde_json::from_str::<GovernancePolicy>(&json).unwrap_err();
    assert!(
        error.to_string().contains("exceeds maximum"),
        "unexpected error: {error}"
    );
    // The same bytes are still readable as the unconstrained wire form.
    near_sdk::serde_json::from_str::<GovernancePolicyWire>(&json).unwrap();
}

#[test]
fn method_policy_cap_is_enforced() {
    let mut policy = policy(1_000);
    for index in 0..MAX_METHOD_POLICIES {
        policy
            .set_method_policy(
                format!("admin_m{index}"),
                Some(method_policy(1, Role::Admin)),
            )
            .unwrap();
    }
    assert_eq!(
        policy.set_method_policy(
            "admin_overflow".to_owned(),
            Some(method_policy(1, Role::Admin))
        ),
        Err(PolicyError::TooManyMethodPolicies {
            maximum: MAX_METHOD_POLICIES,
        })
    );
    // Updating an existing entry at the cap still works.
    policy
        .set_method_policy("admin_m0".to_owned(), Some(method_policy(1, Role::Admin)))
        .unwrap();
}

#[test]
fn reflexive_timelocks_are_independent() {
    let mut policy = policy(1);
    policy
        .set_reflexive_ttl(ReflexiveKind::SelfUpgrade, Nanoseconds::from_secs(100))
        .unwrap();
    policy
        .set_reflexive_ttl(ReflexiveKind::SetRole, Nanoseconds::from_secs(10))
        .unwrap();

    let self_upgrade = Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
        code: UpgradeSource::GlobalHash(Base58CryptoHash::from([7u8; 32])),
        migrate_args: Base64VecU8(vec![]),
    });
    let set_role = Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: "op.near".parse().unwrap(),
        role: Role::Admin,
        set: true,
    });
    assert_eq!(
        self_upgrade.minimum_ttl(&policy),
        Nanoseconds::from_secs(100)
    );
    assert_eq!(set_role.minimum_ttl(&policy), Nanoseconds::from_secs(10));
}

#[test]
fn target_minimum_ttl_uses_resolved_method_policy() {
    let mut policy = policy(100);
    policy
        .set_method_policy(
            "admin_set_manual_trip".to_owned(),
            Some(method_policy(3, Role::ManualTripper)),
        )
        .unwrap();
    assert_eq!(
        target("admin_set_manual_trip", 30).minimum_ttl(&policy),
        Nanoseconds::from_secs(3)
    );
    assert_eq!(
        target("admin_unknown", 30).minimum_ttl(&policy),
        Nanoseconds::from_secs(100)
    );
}

#[test]
fn every_reflexive_op_requires_admin_and_target_roles_resolve() {
    let mut policy = policy(100);
    policy
        .set_method_policy(
            "admin_rearm".to_owned(),
            Some(method_policy(1, Role::CircuitBreakerOperator)),
        )
        .unwrap();

    // Every reflexive op governs governance itself and requires Admin.
    let reflexive = [
        Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
            policy: method_policy(1, Role::Admin),
        }),
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: "admin_upgrade".to_owned(),
            policy: None,
        }),
        Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
            kind: ReflexiveKind::SetRole,
            ttl: Nanoseconds::zero(),
        }),
        Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: "op.near".parse().unwrap(),
            role: Role::Admin,
            set: true,
        }),
    ];
    for operation in reflexive {
        assert_eq!(operation.required_role(&policy), Role::Admin);
    }

    // Target roles come from the resolved policy (override, else conservative default).
    assert_eq!(
        target("admin_rearm", 30).required_role(&policy),
        Role::CircuitBreakerOperator
    );
    assert_eq!(
        target("admin_unknown", 30).required_role(&policy),
        Role::Admin
    );
}

#[test]
fn set_reflexive_ttl_cannot_outrun_the_bucket_it_shortens() {
    let mut policy = policy(1);
    policy
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::from_secs(1))
        .unwrap();
    policy
        .set_reflexive_ttl(ReflexiveKind::SelfUpgrade, Nanoseconds::from_secs(100))
        .unwrap();

    // Shortening self_upgrade must mature under max(set_policy = 1, self_upgrade = 100) = 100, so the
    // long self-upgrade lock protects its own shortening.
    let shorten_self_upgrade = Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
        kind: ReflexiveKind::SelfUpgrade,
        ttl: Nanoseconds::zero(),
    });
    assert_eq!(
        shorten_self_upgrade.minimum_ttl(&policy),
        Nanoseconds::from_secs(100)
    );

    // Editing a bucket shorter than the policy-edit lock still waits at least that lock.
    policy
        .set_reflexive_ttl(ReflexiveKind::SetRole, Nanoseconds::zero())
        .unwrap();
    let edit_set_role = Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
        kind: ReflexiveKind::SetRole,
        ttl: Nanoseconds::from_secs(50),
    });
    assert_eq!(
        edit_set_role.minimum_ttl(&policy),
        Nanoseconds::from_secs(1)
    );
}

#[test]
fn lowering_a_method_or_default_lock_matures_under_that_lock() {
    let mut policy = policy(100); // default_target ttl = 100
    policy
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::from_secs(1))
        .unwrap();
    policy
        .set_method_policy(
            "admin_upgrade".to_owned(),
            Some(method_policy(90, Role::Admin)),
        )
        .unwrap();

    let set_method = |ttl_secs| {
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: "admin_upgrade".to_owned(),
            policy: Some(method_policy(ttl_secs, Role::Admin)),
        })
    };
    // Shortening admin_upgrade's 90s lock matures under max(edit = 1, 90) = 90.
    assert_eq!(
        set_method(0).minimum_ttl(&policy),
        Nanoseconds::from_secs(90)
    );
    // Holding it (or raising) needs only the policy-edit lock.
    assert_eq!(
        set_method(90).minimum_ttl(&policy),
        Nanoseconds::from_secs(1)
    );

    // Adding a shorter override for a previously-unlisted method shortens it from the default (100).
    let list_short = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: "admin_brand_new".to_owned(),
        policy: Some(method_policy(3, Role::Admin)),
    });
    assert_eq!(list_short.minimum_ttl(&policy), Nanoseconds::from_secs(100));

    // Lowering the target default (100) matures under max(1, 100) = 100; raising needs only the lock.
    let lower_default = Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
        policy: method_policy(95, Role::Admin),
    });
    assert_eq!(
        lower_default.minimum_ttl(&policy),
        Nanoseconds::from_secs(100)
    );
    let raise_default = Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
        policy: method_policy(200, Role::Admin),
    });
    assert_eq!(
        raise_default.minimum_ttl(&policy),
        Nanoseconds::from_secs(1)
    );
}

#[test]
fn function_call_validation() {
    assert_eq!(
        Operation::TargetFunctionCall(FunctionCall {
            method_name: "   ".to_owned(),
            args: Base64VecU8(vec![]),
            attached_deposit: U128(0),
            gas: Gas::from_tgas(30),
        })
        .on_create(),
        Err(ValidationError::EmptyFunctionCallMethodName)
    );
    assert_eq!(
        Operation::TargetFunctionCall(FunctionCall {
            method_name: "admin_set_proxy".to_owned(),
            args: Base64VecU8(vec![]),
            attached_deposit: U128(0),
            gas: Gas::from_gas(0),
        })
        .on_create(),
        Err(ValidationError::ZeroFunctionCallGas)
    );
    assert_eq!(target("admin_set_proxy", 30).on_create(), Ok(()));
}

#[test]
fn self_upgrade_rejects_empty_code() {
    let empty = Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
        code: UpgradeSource::Code(Base64VecU8(vec![])),
        migrate_args: Base64VecU8(vec![]),
    });
    assert_eq!(empty.on_create(), Err(ValidationError::EmptyUpgradeCode));
}

#[test]
fn kind_and_method_projection() {
    assert_eq!(
        target("admin_set_proxy", 30).kind(),
        OperationKind::TargetFunctionCall
    );
    assert_eq!(
        target("admin_set_proxy", 30).method().as_deref(),
        Some("admin_set_proxy")
    );
    let set_role = Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: "op.near".parse().unwrap(),
        role: Role::Admin,
        set: true,
    });
    assert_eq!(set_role.kind(), OperationKind::SetRole);
    assert_eq!(set_role.method(), None);
}

#[test]
fn operation_borsh_and_json_round_trip() {
    let operation = target("admin_set_proxy", 30);
    let bytes = near_sdk::borsh::to_vec(&operation).unwrap();
    assert_eq!(
        near_sdk::borsh::from_slice::<Operation>(&bytes).unwrap(),
        operation
    );
    let json = near_sdk::serde_json::to_value(&operation).unwrap();
    assert_eq!(
        near_sdk::serde_json::from_value::<Operation>(json).unwrap(),
        operation
    );
}

/// Pre-restructure JSON is rejected rather than converted: the old typed classification carried its
/// own authorization (`AdminFunctionCall` was Admin-only whatever it called, `SetActionTtl` edited a
/// TTL without touching roles), and silently reinterpreting it under the method-driven policy would
/// create proposals with different privileges than the sender wrote. Old clients get a parse error.
#[test]
fn legacy_json_is_rejected() {
    for legacy in [
        near_sdk::serde_json::json!({
            "SetRole": { "account_id": "op.near", "role": "Admin", "set": true }
        }),
        near_sdk::serde_json::json!({
            "SetActionTtl": { "kind": "SetProxy", "new_ttl": "1" }
        }),
    ] {
        assert!(
            near_sdk::serde_json::from_value::<Operation>(legacy.clone()).is_err(),
            "legacy shape must not deserialize: {legacy}"
        );
    }
}

#[test]
fn governance_policy_round_trip() {
    let mut policy = policy(100);
    policy
        .set_method_policy(
            "admin_upgrade".to_owned(),
            Some(method_policy(90, Role::Admin)),
        )
        .unwrap();
    let bytes = near_sdk::borsh::to_vec(&policy).unwrap();
    assert_eq!(
        near_sdk::borsh::from_slice::<GovernancePolicy>(&bytes).unwrap(),
        policy
    );
}

#[test]
fn legacy_admin_upgrade_maps_to_target_call_with_upgrade_gas() {
    let legacy = LegacyOperation::AdminUpgrade {
        code: UpgradeSource::Code(Base64VecU8(vec![0xde, 0xad])),
        migrate_args: Base64VecU8(vec![0xbe, 0xef]),
    };
    let Operation::TargetFunctionCall(call) = Operation::try_from(legacy).unwrap() else {
        panic!("expected target call");
    };
    assert_eq!(call.method_name, "admin_upgrade");
    assert_eq!(call.gas, GAS_FOR_ADMIN_UPGRADE);
    // args are the JSON of admin_upgrade(code, migrate_args); `UpgradeSource::Code` is a bare base64.
    let args: near_sdk::serde_json::Value = near_sdk::serde_json::from_slice(&call.args.0).unwrap();
    assert_eq!(args["code"], near_sdk::serde_json::json!("3q0="));
    assert_eq!(args["migrate_args"], near_sdk::serde_json::json!("vu8="));
}

#[test]
fn target_builders_apply_gas_default_and_override() {
    let pid = templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]);
    // No override → the method default.
    let default = target::admin_set_proxy(pid, None, None).unwrap();
    assert_eq!(default.gas, target::GAS_FOR_TARGET_DEFAULT);
    // An override lands on the built call.
    let overridden = target::admin_set_proxy(pid, None, Some(Gas::from_tgas(120))).unwrap();
    assert_eq!(overridden.gas, Gas::from_tgas(120));
    // `admin_upgrade` keeps its own (280 Tgas) default.
    let upgrade = target::admin_upgrade(
        UpgradeSource::Code(Base64VecU8(vec![0xde, 0xad])),
        Base64VecU8(vec![]),
        None,
    )
    .unwrap();
    assert_eq!(upgrade.gas, GAS_FOR_ADMIN_UPGRADE);
}

#[test]
fn legacy_set_manual_trip_metadata_is_base64() {
    let legacy = LegacyOperation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0xaa; 32]),
        is_manually_tripped: true,
        metadata: Some(vec![0x01, 0x02, 0x03]),
    };
    let Operation::TargetFunctionCall(call) = Operation::try_from(legacy).unwrap() else {
        panic!("expected target call");
    };
    assert_eq!(call.method_name, "admin_set_manual_trip");
    let args: near_sdk::serde_json::Value = near_sdk::serde_json::from_slice(&call.args.0).unwrap();
    // base64 of [1,2,3]
    assert_eq!(args["metadata"], near_sdk::serde_json::json!("AQID"));
}

#[test]
fn legacy_reflexive_and_ttl_edits_map_through() {
    let set_role = LegacyOperation::SetRole {
        account_id: "op.near".parse().unwrap(),
        role: Role::CircuitBreakerOperator,
        set: false,
    };
    assert_eq!(
        Operation::try_from(set_role).unwrap(),
        Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: "op.near".parse().unwrap(),
            role: Role::CircuitBreakerOperator,
            set: false,
        })
    );

    let target_ttl = LegacyOperation::SetActionTtl {
        kind: LegacyOperationKind::SetProxy,
        new_ttl: Nanoseconds::from_secs(7),
    };
    assert_eq!(
        Operation::try_from(target_ttl).unwrap(),
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: "admin_set_proxy".to_owned(),
            policy: Some(method_policy(7, Role::ProxyConfigurationManager)),
        })
    );

    let reflexive_ttl = LegacyOperation::SetActionTtl {
        kind: LegacyOperationKind::SelfUpgrade,
        new_ttl: Nanoseconds::from_secs(9),
    };
    assert_eq!(
        Operation::try_from(reflexive_ttl).unwrap(),
        Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
            kind: ReflexiveKind::SelfUpgrade,
            ttl: Nanoseconds::from_secs(9),
        })
    );
}

#[test]
fn legacy_ttl_config_seeds_policy_with_conservative_default() {
    let legacy = LegacyTtlConfig {
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
    };
    let policy = legacy.into_policy();

    // default = max of all target ttls (incl. admin_function_call), conservative Admin role.
    assert_eq!(policy.default_target.ttl, Nanoseconds::from_secs(42));
    assert_eq!(policy.default_target.role, Role::Admin);
    // per-method seeds carry the old ttl and natural role; every entry <= default (invariant holds).
    assert_eq!(
        policy.resolve("admin_set_proxy"),
        method_policy(1, Role::ProxyConfigurationManager)
    );
    assert_eq!(
        policy.resolve("admin_rearm"),
        method_policy(6, Role::CircuitBreakerOperator)
    );
    assert_eq!(
        policy.resolve("admin_upgrade"),
        method_policy(42, Role::Admin)
    );
    // reflexive fields carry over; self_upgrade defaults to admin_upgrade.
    assert_eq!(policy.reflexive_ttls.set_policy, Nanoseconds::from_secs(8));
    assert_eq!(policy.reflexive_ttls.set_role, Nanoseconds::from_secs(9));
    assert_eq!(
        policy.reflexive_ttls.self_upgrade,
        Nanoseconds::from_secs(42)
    );
    // the ceiling invariant holds for every seeded entry.
    for entry in policy.method_policies.values() {
        assert!(entry.ttl <= policy.default_target.ttl);
    }
}

/// Why the bring-up policy pre-declares every method: an unlisted method's current lock is
/// `default_target`, so the *first* override for it is measured against the default and is a
/// shortening whenever it is cheaper. Pre-declared at zero, the same end state is a raise instead.
#[test]
fn a_first_override_is_gated_by_the_default_but_hardening_a_declared_method_is_not() {
    let edit = |ttl_secs: u64| {
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: "admin_set_proxy".to_owned(),
            policy: Some(method_policy(ttl_secs, Role::ProxyConfigurationManager)),
        })
    };

    // `policy(300)` leaves every reflexive lock at zero, so the policy-edit lock contributes nothing
    // and any delay below is the shortening rule alone.
    let bare = policy(300);
    assert_eq!(
        edit(100).minimum_ttl(&bare),
        Nanoseconds::from_secs(300),
        "a cheaper first override for an unlisted method matures under default_target"
    );
    // Matching the default is a hold, not a shortening.
    assert_eq!(edit(300).minimum_ttl(&bare), Nanoseconds::zero());

    let mut predeclared = policy(300);
    predeclared
        .set_method_policy(
            "admin_set_proxy".to_owned(),
            Some(method_policy(0, Role::ProxyConfigurationManager)),
        )
        .unwrap();
    assert_eq!(
        edit(100).minimum_ttl(&predeclared),
        Nanoseconds::zero(),
        "raising a method declared at zero is immediate"
    );
}

/// The policies shipped for `governance create --policy-file` have to stay parseable, or operators
/// copy a file the contract will reject at init.
#[test]
fn the_example_policy_files_parse() {
    let steady: GovernancePolicy =
        near_sdk::serde_json::from_str(include_str!("../../../governance-policy.example.json"))
            .unwrap();
    let bootstrap: GovernancePolicy = near_sdk::serde_json::from_str(include_str!(
        "../../../governance-policy.bootstrap.example.json"
    ))
    .unwrap();

    assert_eq!(
        steady.resolve("admin_set_manual_trip").role,
        Role::ManualTripper
    );
    // An unlisted method still falls back to the conservative default.
    assert_eq!(steady.resolve("admin_unknown").role, Role::Admin);
    // Self-upgrade is the effective ceiling on every other lock, so it should be the longest.
    let ttls = steady.reflexive_ttls();
    assert!(ttls.self_upgrade > ttls.set_policy && ttls.self_upgrade > ttls.set_role);
    assert!(ttls.self_upgrade > steady.default_target().ttl);

    // Bring-up policy: same delegation, every lock open.
    assert_eq!(bootstrap.default_target().ttl, Nanoseconds::zero());
    assert_eq!(bootstrap.reflexive_ttls().set_policy, Nanoseconds::zero());
    assert!(bootstrap
        .method_policies()
        .values()
        .all(|entry| entry.ttl == Nanoseconds::zero()));

    // The two files must agree on who may do what; only the timelocks differ. A role added to one
    // and forgotten in the other would silently change delegation across the hardening step.
    let roles = |policy: &GovernancePolicy| -> BTreeMap<String, Role> {
        policy
            .method_policies()
            .iter()
            .map(|(method, entry)| (method.clone(), entry.role))
            .collect()
    };
    assert_eq!(roles(&steady), roles(&bootstrap));
}

/// The migration still converts the v0 typed form; that mapping is faithful because the seeded
/// policy's roles are exactly the v0 ones at the instant it runs.
#[test]
fn legacy_operations_still_convert_for_the_migration() {
    let converted = Operation::try_from(LegacyOperation::SetRole {
        account_id: "op.near".parse().unwrap(),
        role: Role::Admin,
        set: true,
    })
    .unwrap();
    assert_eq!(
        converted,
        Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: "op.near".parse().unwrap(),
            role: Role::Admin,
            set: true,
        })
    );
}
