#![allow(
    clippy::too_many_arguments,
    reason = "mock contract entrypoints mirror external governance policy ABI"
)]

use super::*;
use soroban_sdk::{
    contracttype,
    testutils::{Address as _, Ledger, LedgerInfo},
};
use templar_soroban_shared_types::{GovernanceConfigKind, GovernancePolicyKind};

#[contract]
struct MockVault;

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
enum MockVaultKey {
    Paused,
    Guardian,
    Sentinel,
}

#[contractimpl]
impl MockVault {
    pub fn set_paused(env: Env, _caller: Address, paused: bool) {
        env.storage().instance().set(&MockVaultKey::Paused, &paused);
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&MockVaultKey::Paused)
            .unwrap_or(false)
    }

    pub fn guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::Guardian)
    }

    pub fn sentinel(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::Sentinel)
    }

    pub fn set_governance_config(
        env: Env,
        _caller: Address,
        kind: GovernanceConfigKind,
        primary: Option<Address>,
        many: Option<Vec<Address>>,
        _value_a: Option<i128>,
        _value_b: Option<i128>,
    ) {
        match kind {
            GovernanceConfigKind::Sentinel => {
                let Some(sentinel) = primary else {
                    return;
                };
                env.storage()
                    .instance()
                    .set(&MockVaultKey::Sentinel, &sentinel);
            }
            GovernanceConfigKind::Guardians => {
                let Some(guardians) = many else {
                    return;
                };
                let guardian = if guardians.is_empty() {
                    None
                } else {
                    Some(guardians.get_unchecked(0))
                };
                env.storage()
                    .instance()
                    .set(&MockVaultKey::Guardian, &guardian);
            }
            _ => {}
        }
    }

    pub fn set_governance_policy(
        env: Env,
        _caller: Address,
        kind: GovernancePolicyKind,
        _target_ids: Option<Vec<u32>>,
        mode: Option<u32>,
        accounts: Option<Vec<Address>>,
        _market_id: Option<u32>,
        _cap_group_id: Option<SdkString>,
        _value: Option<i128>,
        _value_b: Option<i128>,
        _value_c: Option<i128>,
    ) {
        if kind == GovernancePolicyKind::Paused {
            let paused = mode.unwrap_or(0) != 0;
            env.storage().instance().set(&MockVaultKey::Paused, &paused);
        }
        if kind == GovernancePolicyKind::Fees {
            let _ = accounts;
        }
    }

    pub fn skim(_env: Env, _caller: Address, _token: Address) {}
}

#[test]
fn sentinel_first_change_immediate_second_timelocked() {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let first = Address::generate(&env);
    let second = Address::generate(&env);

    let id1 = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            first.clone(),
        )
        .unwrap()
    });
    assert_eq!(id1, 1);

    let on_vault = env.as_contract(&vault, || MockVault::sentinel(env.clone()));
    assert_eq!(on_vault, Some(first));

    let id2 = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            second.clone(),
        )
        .unwrap()
    });
    assert_eq!(id2, 2);

    let early = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), id2)
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), id2).unwrap()
    });

    let on_vault = env.as_contract(&vault, || MockVault::sentinel(env.clone()));
    assert_eq!(on_vault, Some(second));
}

#[test]
fn pause_immediate_unpause_timelocked() {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let pause_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_paused(env.clone(), admin.clone(), true).unwrap()
    });
    assert_eq!(pause_id, 1);
    let paused = env.as_contract(&vault, || MockVault::is_paused(env.clone()));
    assert!(paused);
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    let unpause_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_paused(env.clone(), admin.clone(), false)
            .unwrap()
    });
    assert_eq!(unpause_id, 2);

    let early = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), unpause_id)
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), unpause_id).unwrap()
    });
    let paused = env.as_contract(&vault, || MockVault::is_paused(env.clone()));
    assert!(!paused);
}

#[test]
fn revoke_kind_removes_all_matching() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_curator(
            env.clone(),
            admin.clone(),
            Address::generate(&env),
        )
        .unwrap();
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_curator(
            env.clone(),
            admin.clone(),
            Address::generate(&env),
        )
        .unwrap();
    });

    let removed = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::revoke_kind(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Curator,
        )
        .unwrap()
    });
    assert_eq!(removed, 2);

    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);
}

#[test]
fn timelock_config_increase_immediate_decrease_timelocked() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_timelock(
            env.clone(),
            admin.clone(),
            TimelockKind::Curator,
            6_000_000_000,
        )
        .unwrap();
    });

    let updated = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::timelock_ns(env.clone(), TimelockKind::Curator)
    });
    assert_eq!(updated, 6_000_000_000);

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_timelock(
            env.clone(),
            admin.clone(),
            TimelockKind::Curator,
            4_000_000_000,
        )
        .unwrap();
    });

    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 1);
}

#[test]
fn other_action_approval_and_consume() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let key = Symbol::new(&env, "market_remove");
    let payload_hash = BytesN::from_array(&env, &[7u8; 32]);

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_other(
            env.clone(),
            admin.clone(),
            key.clone(),
            payload_hash.clone(),
        )
        .unwrap()
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), proposal_id).unwrap()
    });

    let approved = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::check_other(env.clone(), key.clone(), payload_hash.clone())
    });
    assert!(approved);

    let unauthorized = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::consume_other(
            env.clone(),
            admin.clone(),
            key.clone(),
            payload_hash.clone(),
        )
    });
    assert_eq!(unauthorized, Err(GovernanceError::Unauthorized));

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::consume_other(
            env.clone(),
            vault.clone(),
            key.clone(),
            payload_hash.clone(),
        )
        .unwrap();
    });

    let approved_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::check_other(env.clone(), key, payload_hash)
    });
    assert!(!approved_after);
}

#[test]
fn abdicated_action_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let abdicated = Symbol::new(&env, "submit_cap");
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::abdicate(env.clone(), admin.clone(), abdicated.clone())
            .unwrap();
    });

    let submit_result = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_cap(env.clone(), admin.clone(), 7, 10)
    });
    assert_eq!(submit_result, Err(GovernanceError::Abdicated));
}

#[test]
fn cap_action_is_timelocked_and_accepts_after_maturity() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_cap(env.clone(), admin.clone(), 3, 10).unwrap()
    });

    let early = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), proposal_id)
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), proposal_id).unwrap()
    });
}
