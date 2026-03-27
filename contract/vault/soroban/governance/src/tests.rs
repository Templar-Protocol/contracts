use super::*;
use soroban_sdk::{
    contracttype,
    testutils::{Address as _, Ledger, LedgerInfo},
};

#[contract]
struct MockVault;

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
enum MockVaultKey {
    Paused,
    Guardian,
    Sentinel,
    Fees,
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

    pub fn set_curator(_env: Env, _caller: Address, _new_curator: Address) {}

    pub fn set_governance(_env: Env, _caller: Address, _governance: Address) {}

    pub fn set_supply_queue(_env: Env, _caller: Address, _target_ids: Vec<u32>) {}

    pub fn set_fees(
        env: Env,
        _caller: Address,
        performance_fee_wad: i128,
        performance_recipient: Address,
        management_fee_wad: i128,
        management_recipient: Address,
        max_growth_rate_wad: Option<i128>,
    ) {
        env.storage().instance().set(
            &MockVaultKey::Fees,
            &FeeParams {
                performance_fee_wad,
                performance_recipient,
                management_fee_wad,
                management_recipient,
                max_growth_rate_wad,
            },
        );
    }

    pub fn fees(env: Env) -> Option<FeeParams> {
        env.storage().instance().get(&MockVaultKey::Fees)
    }

    pub fn set_restrictions(_env: Env, _caller: Address, _mode: u32, _accounts: Vec<Address>) {}

    pub fn set_cap(_env: Env, _caller: Address, _market_id: u32, _new_cap: i128) {}

    pub fn remove_market(_env: Env, _caller: Address, _market_id: u32) {}

    pub fn set_group_cap(_env: Env, _caller: Address, _cap_group_id: SdkString, _new_cap: i128) {}

    pub fn set_group_rel_cap(
        _env: Env,
        _caller: Address,
        _cap_group_id: SdkString,
        _new_relative_cap_wad: i128,
    ) {
    }

    pub fn set_group_member(
        _env: Env,
        _caller: Address,
        _market_id: u32,
        _cap_group_id: SdkString,
    ) {
    }

    pub fn set_skim_recipient(_env: Env, _caller: Address, _recipient: Address) {}

    pub fn skim(_env: Env, _caller: Address, _token: Address) {}

    pub fn set_guardians(env: Env, _caller: Address, guardians: Vec<Address>) {
        let guardian = if guardians.is_empty() {
            None
        } else {
            Some(guardians.get_unchecked(0))
        };
        env.storage()
            .instance()
            .set(&MockVaultKey::Guardian, &guardian);
    }

    pub fn set_sentinel(env: Env, _caller: Address, sentinel: Address) {
        env.storage()
            .instance()
            .set(&MockVaultKey::Sentinel, &sentinel);
    }
}

#[test]
fn sentinel_first_change_immediate_second_timelocked() {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
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
        protocol_version: 23,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), proposal_id).unwrap()
    });
}

#[test]
fn fee_decrease_applies_immediately_when_recipients_are_unchanged() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let perf_recipient = Address::generate(&env);
    let mgmt_recipient = Address::generate(&env);

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            100_000_000_000_000_000,
            perf_recipient.clone(),
            50_000_000_000_000_000,
            mgmt_recipient.clone(),
            None,
        )
        .unwrap()
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 23,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_fees(env.clone(), admin.clone()).unwrap();
    });

    let pending_before = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_fees_valid_at(env.clone())
    });
    assert_eq!(pending_before, None);

    let on_vault_before = env
        .as_contract(&vault, || MockVault::fees(env.clone()))
        .unwrap();
    assert_eq!(on_vault_before.performance_fee_wad, 100_000_000_000_000_000);
    assert_eq!(on_vault_before.management_fee_wad, 50_000_000_000_000_000);

    let immediate_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            0,
            perf_recipient.clone(),
            0,
            mgmt_recipient.clone(),
            None,
        )
        .unwrap()
    });
    assert_eq!(proposal_id, 1);
    assert_eq!(immediate_id, 2);

    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_fees_valid_at(env.clone())
    });
    assert_eq!(pending, None);

    let on_vault = env
        .as_contract(&vault, || MockVault::fees(env.clone()))
        .unwrap();
    assert_eq!(on_vault.performance_fee_wad, 0);
    assert_eq!(on_vault.management_fee_wad, 0);
}

#[test]
fn fee_increase_uses_fee_specific_pending_accept_and_revoke() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = env.register(MockVault, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&admin, &vault, &(5_000_000_000u64)),
    );

    let perf_recipient = Address::generate(&env);
    let mgmt_recipient = Address::generate(&env);

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            100_000_000_000_000_000,
            perf_recipient.clone(),
            0,
            mgmt_recipient.clone(),
            None,
        )
        .unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 23,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_fees(env.clone(), admin.clone()).unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: 110,
        protocol_version: 23,
        ..Default::default()
    });

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            200_000_000_000_000_000,
            perf_recipient.clone(),
            0,
            mgmt_recipient.clone(),
            None,
        )
        .unwrap()
    });
    assert_eq!(proposal_id, 2);

    let valid_at = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_fees_valid_at(env.clone())
    });
    assert_eq!(valid_at, Some(115_000_000_000));

    let early = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_fees(env.clone(), admin.clone())
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));

    let revoked = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::revoke_pending_fees(env.clone(), admin.clone()).unwrap()
    });
    assert_eq!(revoked, 1);

    let pending_after_revoke = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_fees_valid_at(env.clone())
    });
    assert_eq!(pending_after_revoke, None);

    let not_found = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_fees(env.clone(), admin.clone())
    });
    assert_eq!(not_found, Err(GovernanceError::ProposalNotFound));

    let reproposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            200_000_000_000_000_000,
            perf_recipient.clone(),
            0,
            mgmt_recipient.clone(),
            None,
        )
        .unwrap()
    });
    assert_eq!(reproposal_id, 3);

    let duplicate = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            300_000_000_000_000_000,
            perf_recipient.clone(),
            0,
            mgmt_recipient.clone(),
            None,
        )
    });
    assert_eq!(duplicate, Err(GovernanceError::DuplicatePending));

    env.ledger().set(LedgerInfo {
        timestamp: 116,
        protocol_version: 23,
        ..Default::default()
    });

    let accepted_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_fees(env.clone(), admin.clone()).unwrap()
    });
    assert_eq!(accepted_id, reproposal_id);

    let pending_after_accept = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_fees_valid_at(env.clone())
    });
    assert_eq!(pending_after_accept, None);

    let on_vault = env
        .as_contract(&vault, || MockVault::fees(env.clone()))
        .unwrap();
    assert_eq!(on_vault.performance_fee_wad, 200_000_000_000_000_000);
    assert_eq!(on_vault.management_fee_wad, 0);
}
