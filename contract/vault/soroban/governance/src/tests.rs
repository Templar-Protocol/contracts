#![allow(
    clippy::too_many_arguments,
    reason = "mock contract entrypoints mirror external governance policy ABI"
)]

use super::*;

use soroban_sdk::{
    contracttype,
    testutils::{Address as _, Ledger, LedgerInfo},
    Bytes, String as SdkString,
};
use templar_soroban_shared_types::{
    GovernanceCommand, GOVERNANCE_CONFIG_KIND_CURATOR, GOVERNANCE_CONFIG_KIND_GOVERNANCE,
    GOVERNANCE_CONFIG_KIND_SENTINEL, GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
    GOVERNANCE_POLICY_KIND_CAP, GOVERNANCE_POLICY_KIND_FEES, GOVERNANCE_POLICY_KIND_GROUP,
    GOVERNANCE_POLICY_KIND_PAUSED, GOVERNANCE_POLICY_KIND_REMOVE_MARKET,
    GOVERNANCE_POLICY_KIND_RESTRICTIONS, GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};

#[contract]
struct MockVault;

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
enum MockVaultKey {
    Paused,
    Sentinel,
    Curator,
    Governance,
    SkimRecipient,
    SupplyQueue,
    LastFeeAccounts,
    RestrictionMode,
    RestrictionAccounts,
    LastCapMarketId,
    LastCapValue,
    LastRemoveMarketId,
    LastGroupCapGroupId,
    LastGroupCapValue,
    LastGroupRelCapGroupId,
    LastGroupRelCapValue,
    LastGroupMemberMarketId,
    LastGroupMemberGroupId,
    LastSkimToken,
}

#[contractimpl]
#[allow(
    dead_code,
    reason = "mock entrypoints mirror older governance ABI helpers"
)]
impl MockVault {
    fn set_curator(env: Env, caller: Address, new_curator: Address) {
        Self::set_governance_config(
            env,
            caller,
            GOVERNANCE_CONFIG_KIND_CURATOR,
            Some(new_curator),
            None,
            None,
            None,
        );
    }

    fn set_governance(env: Env, caller: Address, governance: Address) {
        Self::set_governance_config(
            env,
            caller,
            GOVERNANCE_CONFIG_KIND_GOVERNANCE,
            Some(governance),
            None,
            None,
            None,
        );
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Mirrors runtime governance policy ABI"
    )]
    fn set_governance_policy(
        env: Env,
        caller: Address,
        kind: u32,
        target_ids: Option<Vec<u32>>,
        mode: Option<u32>,
        accounts: Option<Vec<Address>>,
        market_id: Option<u32>,
        cap_group_id: Option<SdkString>,
        value: Option<i128>,
        value_b: Option<i128>,
        value_c: Option<i128>,
    ) {
        Self::set_governance_policy_impl(
            env,
            caller,
            kind,
            target_ids,
            mode,
            accounts,
            market_id,
            cap_group_id,
            value,
            value_b,
            value_c,
        );
    }

    fn submit_sentinel(env: Env, caller: Address, sentinel: Address) {
        Self::set_governance_config(
            env,
            caller,
            GOVERNANCE_CONFIG_KIND_SENTINEL,
            Some(sentinel),
            None,
            None,
            None,
        );
    }

    fn submit_cap(env: Env, caller: Address, market_id: u32, value: i128) {
        Self::set_governance_policy_impl(
            env,
            caller,
            GOVERNANCE_POLICY_KIND_CAP,
            None,
            None,
            None,
            Some(market_id),
            None,
            Some(value),
            None,
            None,
        );
    }

    fn submit_market_removal(env: Env, caller: Address, market_id: u32) {
        Self::set_governance_policy_impl(
            env,
            caller,
            GOVERNANCE_POLICY_KIND_REMOVE_MARKET,
            None,
            None,
            None,
            Some(market_id),
            None,
            None,
            None,
            None,
        );
    }

    fn submit_cap_group_update(
        env: Env,
        caller: Address,
        mode: u32,
        market_id: Option<u32>,
        cap_group_id: Option<SdkString>,
        value: Option<i128>,
    ) {
        Self::set_governance_policy_impl(
            env,
            caller,
            GOVERNANCE_POLICY_KIND_GROUP,
            None,
            Some(mode),
            None,
            market_id,
            cap_group_id,
            value,
            None,
            None,
        );
    }

    fn set_skim_recipient(env: Env, caller: Address, recipient: Address) {
        Self::set_governance_config(
            env,
            caller,
            GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT,
            Some(recipient),
            None,
            None,
            None,
        );
    }

    pub fn execute_governance(env: Env, caller: Address, payload: Bytes) {
        let command = match GovernanceCommand::decode(&payload.to_alloc_vec()) {
            Ok(command) => command,
            Err(_) => panic!("decode governance command failed"),
        };

        match command {
            GovernanceCommand::SetGovernanceConfig {
                kind,
                primary,
                many,
                value_a,
                value_b,
            } => {
                let primary = primary.map(|value| sdk_address(&env, &value));
                let many = many.map(|values| sdk_address_vec(&env, &values));
                Self::set_governance_config(
                    env.clone(),
                    caller,
                    kind,
                    primary,
                    many,
                    value_a,
                    value_b,
                );
            }
            GovernanceCommand::SetGovernancePolicy {
                kind,
                target_ids,
                mode,
                accounts,
                market_id,
                cap_group_id,
                value,
                value_b,
                value_c,
            } => {
                let target_ids = target_ids.map(|values| sdk_u32_vec(&env, &values));
                let accounts = accounts.map(|values| sdk_address_vec(&env, &values));
                let cap_group_id = cap_group_id.map(|value| SdkString::from_str(&env, &value));
                Self::set_governance_policy(
                    env.clone(),
                    caller,
                    kind,
                    target_ids,
                    mode,
                    accounts,
                    market_id,
                    cap_group_id,
                    value,
                    value_b,
                    value_c,
                );
            }
            GovernanceCommand::Skim { token } => {
                Self::skim(env.clone(), caller, sdk_address(&env, &token))
            }
        }
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&MockVaultKey::Paused)
            .unwrap_or(false)
    }

    pub fn sentinel(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::Sentinel)
    }

    pub fn curator(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::Curator)
    }

    pub fn governance(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::Governance)
    }

    pub fn skim_recipient(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::SkimRecipient)
    }

    pub fn supply_queue(env: Env) -> Vec<u32> {
        env.storage()
            .instance()
            .get(&MockVaultKey::SupplyQueue)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn last_fee_accounts(env: Env) -> Option<Vec<Address>> {
        env.storage().instance().get(&MockVaultKey::LastFeeAccounts)
    }

    pub fn restriction_mode(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&MockVaultKey::RestrictionMode)
            .unwrap_or(0)
    }

    pub fn restriction_accounts(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&MockVaultKey::RestrictionAccounts)
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn last_cap_market_id(env: Env) -> Option<u32> {
        env.storage().instance().get(&MockVaultKey::LastCapMarketId)
    }

    pub fn last_cap_value(env: Env) -> Option<i128> {
        env.storage().instance().get(&MockVaultKey::LastCapValue)
    }

    pub fn last_remove_market_id(env: Env) -> Option<u32> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastRemoveMarketId)
    }

    pub fn last_group_cap_group_id(env: Env) -> Option<SdkString> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastGroupCapGroupId)
    }

    pub fn last_group_cap_value(env: Env) -> Option<i128> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastGroupCapValue)
    }

    pub fn last_group_rel_cap_group_id(env: Env) -> Option<SdkString> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastGroupRelCapGroupId)
    }

    pub fn last_group_rel_cap_value(env: Env) -> Option<i128> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastGroupRelCapValue)
    }

    pub fn last_group_member_market_id(env: Env) -> Option<u32> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastGroupMemberMarketId)
    }

    pub fn last_group_member_group_id(env: Env) -> Option<SdkString> {
        env.storage()
            .instance()
            .get(&MockVaultKey::LastGroupMemberGroupId)
    }

    pub fn last_skim_token(env: Env) -> Option<Address> {
        env.storage().instance().get(&MockVaultKey::LastSkimToken)
    }

    fn set_governance_config(
        env: Env,
        _caller: Address,
        kind: u32,
        primary: Option<Address>,
        _many: Option<Vec<Address>>,
        _value_a: Option<i128>,
        _value_b: Option<i128>,
    ) {
        match kind {
            GOVERNANCE_CONFIG_KIND_SENTINEL => {
                let Some(sentinel) = primary else {
                    return;
                };
                env.storage()
                    .instance()
                    .set(&MockVaultKey::Sentinel, &sentinel);
            }
            GOVERNANCE_CONFIG_KIND_CURATOR => {
                if let Some(curator) = primary {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::Curator, &curator);
                }
            }
            GOVERNANCE_CONFIG_KIND_GOVERNANCE => {
                if let Some(governance) = primary {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::Governance, &governance);
                }
            }
            GOVERNANCE_CONFIG_KIND_SKIM_RECIPIENT => {
                if let Some(recipient) = primary {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::SkimRecipient, &recipient);
                }
            }
            _ => {}
        }
    }

    fn set_governance_policy_impl(
        env: Env,
        _caller: Address,
        kind: u32,
        target_ids: Option<Vec<u32>>,
        mode: Option<u32>,
        accounts: Option<Vec<Address>>,
        market_id: Option<u32>,
        cap_group_id: Option<SdkString>,
        value: Option<i128>,
        _value_b: Option<i128>,
        _value_c: Option<i128>,
    ) {
        if kind == GOVERNANCE_POLICY_KIND_PAUSED {
            let paused = mode.unwrap_or(0) != 0;
            env.storage().instance().set(&MockVaultKey::Paused, &paused);
        }
        if kind == GOVERNANCE_POLICY_KIND_FEES {
            env.storage()
                .instance()
                .set(&MockVaultKey::LastFeeAccounts, &accounts);
        }
        if kind == GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE {
            if let Some(ids) = target_ids {
                env.storage()
                    .instance()
                    .set(&MockVaultKey::SupplyQueue, &ids);
            }
        }
        if kind == GOVERNANCE_POLICY_KIND_RESTRICTIONS {
            if let Some(m) = mode {
                env.storage()
                    .instance()
                    .set(&MockVaultKey::RestrictionMode, &m);
            }
            if let Some(accs) = accounts {
                env.storage()
                    .instance()
                    .set(&MockVaultKey::RestrictionAccounts, &accs);
            }
        }
        if kind == GOVERNANCE_POLICY_KIND_CAP {
            if let Some(mid) = market_id {
                env.storage()
                    .instance()
                    .set(&MockVaultKey::LastCapMarketId, &mid);
            }
            if let Some(v) = value {
                env.storage()
                    .instance()
                    .set(&MockVaultKey::LastCapValue, &v);
            }
        }
        if kind == GOVERNANCE_POLICY_KIND_REMOVE_MARKET {
            if let Some(mid) = market_id {
                env.storage()
                    .instance()
                    .set(&MockVaultKey::LastRemoveMarketId, &mid);
            }
        }
        if kind == GOVERNANCE_POLICY_KIND_GROUP {
            let mode_val = mode.unwrap_or(0);
            if mode_val == 0 {
                // SetGroupCap
                if let Some(group_id) = cap_group_id.clone() {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::LastGroupCapGroupId, &group_id);
                }
                if let Some(v) = value {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::LastGroupCapValue, &v);
                }
            } else if mode_val == 1 {
                // SetGroupRelCap
                if let Some(group_id) = cap_group_id.clone() {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::LastGroupRelCapGroupId, &group_id);
                }
                if let Some(v) = value {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::LastGroupRelCapValue, &v);
                }
            } else if mode_val == 2 {
                // SetGroupMember
                if let Some(mid) = market_id {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::LastGroupMemberMarketId, &mid);
                }
                if let Some(group_id) = cap_group_id.clone() {
                    env.storage()
                        .instance()
                        .set(&MockVaultKey::LastGroupMemberGroupId, &group_id);
                }
            }
        }
    }

    fn skim(env: Env, _caller: Address, token: Address) {
        env.storage()
            .instance()
            .set(&MockVaultKey::LastSkimToken, &token);
    }
}

fn sdk_address(env: &Env, value: &AllocString) -> Address {
    Address::from_str(env, value)
}

fn sdk_address_vec(env: &Env, values: &[AllocString]) -> Vec<Address> {
    let mut addresses = Vec::new(env);
    for value in values {
        addresses.push_back(sdk_address(env, value));
    }
    addresses
}

fn sdk_u32_vec(env: &Env, values: &[u32]) -> Vec<u32> {
    let mut entries = Vec::new(env);
    for value in values {
        entries.push_back(*value);
    }
    entries
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
    env.as_contract(&governance, || {
        assert!(!env.storage().persistent().has(&DataKey::PendingPageIndex));
    });

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
    assert_eq!(removed, 1);

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

const TEST_MAX_PENDING_PROPOSALS: u32 = 64;

fn fill_pending_other_queue(
    env: &Env,
    governance: &Address,
    admin: &Address,
) -> (Symbol, BytesN<32>) {
    let mut first_key = None;
    let mut first_hash = None;

    for index in 0..TEST_MAX_PENDING_PROPOSALS {
        let key = Symbol::new(env, "queue_cap");
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0..4].copy_from_slice(&index.to_be_bytes());
        let payload_hash = BytesN::from_array(env, &hash_bytes);

        if index == 0 {
            first_key = Some(key.clone());
            first_hash = Some(payload_hash.clone());
        }

        env.as_contract(governance, || {
            SorobanVaultGovernanceContract::submit_other(
                env.clone(),
                admin.clone(),
                key,
                payload_hash,
            )
            .unwrap();
        });
    }

    (first_key.unwrap(), first_hash.unwrap())
}

#[test]
fn pending_queue_cap_rejects_distinct_timelocked_proposals() {
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

    fill_pending_other_queue(&env, &governance, &admin);
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), TEST_MAX_PENDING_PROPOSALS);
    env.as_contract(&governance, || {
        let pages: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::PendingPageIndex)
            .expect("pending page index should be stored");
        let mut seen = 0u32;
        for page in pages.iter() {
            let entries: Vec<StoredPending> = env
                .storage()
                .persistent()
                .get(&DataKey::PendingPage(page))
                .expect("pending page should be stored");
            assert!(
                !entries.is_empty(),
                "indexed pending page must not be empty"
            );
            assert!(entries.len() <= 16, "pending page exceeds page size");
            for entry in entries.iter() {
                assert_eq!(entry.id / 16, page);
                seen += 1;
            }
        }
        assert_eq!(seen, TEST_MAX_PENDING_PROPOSALS);
    });

    let overflow_hash = BytesN::from_array(&env, &[0xff; 32]);
    let overflow = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_other(
            env.clone(),
            admin.clone(),
            Symbol::new(&env, "overflow"),
            overflow_hash,
        )
    });
    assert_eq!(overflow, Err(GovernanceError::InvalidInput));

    let pending_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_after.len(), TEST_MAX_PENDING_PROPOSALS);
}

#[test]
fn pending_queue_cap_allows_replacement_at_capacity() {
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

    let (existing_key, existing_hash) = fill_pending_other_queue(&env, &governance, &admin);
    let replacement = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_other(
            env.clone(),
            admin.clone(),
            existing_key,
            existing_hash,
        )
    });
    assert!(replacement.is_ok());

    let pending_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_after.len(), TEST_MAX_PENDING_PROPOSALS);
}

#[test]
fn pending_queue_cap_does_not_block_immediate_pause() {
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

    fill_pending_other_queue(&env, &governance, &admin);
    let pause = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_paused(env.clone(), admin.clone(), true)
    });
    assert!(pause.is_ok());

    let pending_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_after.len(), TEST_MAX_PENDING_PROPOSALS);
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

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::abdicate(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Cap,
        )
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

#[test]
fn accepted_cap_updates_mirror_for_future_decisions() {
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
    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), proposal_id).unwrap()
    });

    let duplicate = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_cap(env.clone(), admin.clone(), 3, 10)
    });
    assert_eq!(duplicate, Err(GovernanceError::NoChange));

    let _immediate_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_cap(env.clone(), admin.clone(), 3, 5).unwrap()
    });
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    let increase_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_cap(env.clone(), admin.clone(), 3, 20).unwrap()
    });
    let early = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), increase_id)
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));
}

#[test]
fn relative_cap_addition_is_immediate_and_removal_is_timelocked() {
    assert_eq!(
        TimelockDecision::from_relative_cap_change(None, Some(templar_vault_kernel::Wad::from(1))),
        Ok(TimelockDecision::Immediate)
    );
    assert_eq!(
        TimelockDecision::from_relative_cap_change(Some(templar_vault_kernel::Wad::from(1)), None,),
        Ok(TimelockDecision::Timelocked)
    );
}

#[test]
fn empty_group_member_string_is_treated_as_membership_removal() {
    let empty = SdkString::from_str(&Env::default(), "");
    let proposed = if empty.is_empty() { None } else { Some(&empty) };

    assert_eq!(
        TimelockDecision::from_membership_assignment_change::<SdkString>(Some(&empty), proposed),
        Ok(TimelockDecision::Timelocked)
    );
}

#[test]
fn cap_group_membership_clear_uses_mirrored_current_membership() {
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
    let group = SdkString::from_str(&env, "senior");
    let empty = SdkString::from_str(&env, "");

    let assign_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_member(
            env.clone(),
            admin.clone(),
            7,
            group,
        )
        .unwrap()
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), assign_id).unwrap()
    });

    let clear_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_member(
            env.clone(),
            admin.clone(),
            7,
            empty.clone(),
        )
        .unwrap()
    });

    env.ledger().set(LedgerInfo {
        timestamp: 112,
        protocol_version: 25,
        ..Default::default()
    });
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), clear_id).unwrap()
    });

    let duplicate_clear = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_member(env.clone(), admin, 7, empty)
    });
    assert_eq!(duplicate_clear, Err(GovernanceError::NoChange));
}

#[test]
fn governance_change_is_timelocked_and_routes_to_vault() {
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

    let new_governance = Address::generate(&env);

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_governance(
            env.clone(),
            admin.clone(),
            new_governance.clone(),
        )
        .unwrap()
    });

    let on_vault_before = env.as_contract(&vault, || MockVault::governance(env.clone()));
    assert_eq!(on_vault_before, None);

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

    let on_vault_after = env.as_contract(&vault, || MockVault::governance(env.clone()));
    assert_eq!(on_vault_after, Some(new_governance));
}

#[test]
fn supply_queue_submission_routes_to_vault() {
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

    let target_ids = sdk_u32_vec(&env, &[1u32, 2u32, 3u32]);

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_supply_queue(
            env.clone(),
            admin.clone(),
            target_ids.clone(),
        )
        .unwrap()
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

    let on_vault = env.as_contract(&vault, || MockVault::supply_queue(env.clone()));
    assert_eq!(on_vault, target_ids);
}

#[test]
fn fee_decrease_applies_immediately_when_recipients_unchanged() {
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

    let performance_recipient = Address::generate(&env);
    let management_recipient = Address::generate(&env);

    // First set initial fees with recipients
    let _ = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            100_000_000_000_000_000i128, // 10% performance fee
            performance_recipient.clone(),
            50_000_000_000_000_000i128, // 5% management fee
            management_recipient.clone(),
            None,
        )
        .unwrap()
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_kind(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Fees,
        )
        .unwrap()
    });

    // Now decrease performance fee with same recipients - should be immediate
    env.ledger().set(LedgerInfo {
        timestamp: 200,
        protocol_version: 25,
        ..Default::default()
    });

    let _decrease_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            50_000_000_000_000_000i128, // 5% performance fee (decreased)
            performance_recipient.clone(),
            50_000_000_000_000_000i128, // same management fee
            management_recipient.clone(),
            None,
        )
        .unwrap()
    });

    // Fee decrease should apply immediately without pending
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    // Verify the fee accounts were routed to vault
    let fee_accounts = env.as_contract(&vault, || MockVault::last_fee_accounts(env.clone()));
    assert!(fee_accounts.is_some());
    let accounts = fee_accounts.unwrap();
    assert_eq!(accounts.len(), 2);
}

#[test]
fn fee_increase_uses_fee_specific_pending_accept_and_revoke() {
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

    let performance_recipient = Address::generate(&env);
    let management_recipient = Address::generate(&env);

    // Set initial fees
    let _ = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            50_000_000_000_000_000i128, // 5% performance fee
            performance_recipient.clone(),
            50_000_000_000_000_000i128, // 5% management fee
            management_recipient.clone(),
            None,
        )
        .unwrap()
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_kind(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Fees,
        )
        .unwrap()
    });

    // Now increase performance fee - should be timelocked
    env.ledger().set(LedgerInfo {
        timestamp: 200,
        protocol_version: 25,
        ..Default::default()
    });

    let increase_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            100_000_000_000_000_000i128, // 10% performance fee (increased)
            performance_recipient.clone(),
            50_000_000_000_000_000i128, // same management fee
            management_recipient.clone(),
            None,
        )
        .unwrap()
    });

    // Fee increase should create pending proposal
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 1);

    // Accept using accept_kind for Fees
    env.ledger().set(LedgerInfo {
        timestamp: 206,
        protocol_version: 25,
        ..Default::default()
    });

    let accepted_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept_kind(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Fees,
        )
        .unwrap()
    });
    assert_eq!(accepted_id, increase_id);

    // Verify pending is cleared
    let pending_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_after.len(), 0);
}

#[test]
fn restrictions_tightening_is_immediate_relaxation_is_timelocked() {
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

    let account1 = Address::generate(&env);
    let account2 = Address::generate(&env);

    // Start with no restrictions (mode 0 = None)
    // Current governance logic applies this change immediately.
    let mut accounts = Vec::new(&env);
    accounts.push_back(account1.clone());
    accounts.push_back(account2.clone());
    let _proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_restrictions(
            env.clone(),
            admin.clone(),
            1, // Blacklist mode
            accounts.clone(),
        )
        .unwrap()
    });

    // Should apply immediately
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    let mode_on_vault = env.as_contract(&vault, || MockVault::restriction_mode(env.clone()));
    assert_eq!(mode_on_vault, 1);
    let accounts_on_vault =
        env.as_contract(&vault, || MockVault::restriction_accounts(env.clone()));
    assert_eq!(accounts_on_vault.len(), 2);

    // Now relax to None - current governance logic timelocks this change
    env.ledger().set(LedgerInfo {
        timestamp: 200,
        protocol_version: 25,
        ..Default::default()
    });

    let relax_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_restrictions(
            env.clone(),
            admin.clone(),
            0, // None mode (relaxation)
            Vec::new(&env),
        )
        .unwrap()
    });

    // Should be pending
    let pending_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_after.len(), 1);

    env.ledger().set(LedgerInfo {
        timestamp: 206,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::accept(env.clone(), admin.clone(), relax_id).unwrap()
    });

    let mode_after = env.as_contract(&vault, || MockVault::restriction_mode(env.clone()));
    assert_eq!(mode_after, 0);
}

#[test]
fn skim_recipient_change_is_timelocked() {
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

    let recipient = Address::generate(&env);

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_skim_recipient(
            env.clone(),
            admin.clone(),
            recipient.clone(),
        )
        .unwrap()
    });

    // Should be timelocked
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 1);

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

    let on_vault = env.as_contract(&vault, || MockVault::skim_recipient(env.clone()));
    assert_eq!(on_vault, Some(recipient));
}

#[test]
fn skim_action_is_immediate_and_routes_token_to_vault() {
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

    let token = Address::generate(&env);

    let _skim_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_skim(env.clone(), admin.clone(), token.clone())
            .unwrap()
    });

    // Skim should apply immediately
    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    let on_vault = env.as_contract(&vault, || MockVault::last_skim_token(env.clone()));
    assert_eq!(on_vault, Some(token));
}

#[test]
fn remove_market_is_timelocked_and_routes_to_vault() {
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

    let market_id = 7u32;

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_remove_market(env.clone(), admin.clone(), market_id)
            .unwrap()
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

    let on_vault = env.as_contract(&vault, || MockVault::last_remove_market_id(env.clone()));
    assert_eq!(on_vault, Some(market_id));
}

#[test]
fn group_cap_is_immediate_and_routes_to_vault() {
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

    let group_id = SdkString::from_str(&env, "group-a");
    let new_cap = 1_000_000i128;

    let _proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_cap(
            env.clone(),
            admin.clone(),
            group_id.clone(),
            new_cap,
        )
        .unwrap()
    });

    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    let on_vault_id = env.as_contract(&vault, || MockVault::last_group_cap_group_id(env.clone()));
    assert_eq!(on_vault_id, Some(group_id));
    let on_vault_value = env.as_contract(&vault, || MockVault::last_group_cap_value(env.clone()));
    assert_eq!(on_vault_value, Some(new_cap));
}

#[test]
fn group_cap_raise_uses_mirrored_current_cap_and_is_timelocked() {
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

    let group_id = SdkString::from_str(&env, "group-a");
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_cap(
            env.clone(),
            admin.clone(),
            group_id.clone(),
            1_000,
        )
        .unwrap()
    });

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_cap(
            env.clone(),
            admin.clone(),
            group_id.clone(),
            2_000,
        )
        .unwrap()
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

    let on_vault_value = env.as_contract(&vault, || MockVault::last_group_cap_value(env.clone()));
    assert_eq!(on_vault_value, Some(2_000));
}

#[test]
fn group_rel_cap_is_immediate_and_routes_to_vault() {
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

    let group_id = SdkString::from_str(&env, "group-b");
    let rel_cap_wad = 500_000_000_000_000_000i128; // 0.5 wad

    let _proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_rel_cap(
            env.clone(),
            admin.clone(),
            group_id.clone(),
            rel_cap_wad,
        )
        .unwrap()
    });

    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);

    let on_vault_id = env.as_contract(&vault, || {
        MockVault::last_group_rel_cap_group_id(env.clone())
    });
    assert_eq!(on_vault_id, Some(group_id));
    let on_vault_value =
        env.as_contract(&vault, || MockVault::last_group_rel_cap_value(env.clone()));
    assert_eq!(on_vault_value, Some(rel_cap_wad));
}

#[test]
fn group_relative_cap_raise_uses_mirrored_current_cap_and_is_timelocked() {
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

    let group_id = SdkString::from_str(&env, "group-b");
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_rel_cap(
            env.clone(),
            admin.clone(),
            group_id.clone(),
            500_000_000_000_000_000,
        )
        .unwrap()
    });

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_rel_cap(
            env.clone(),
            admin.clone(),
            group_id.clone(),
            750_000_000_000_000_000,
        )
        .unwrap()
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

    let on_vault_value =
        env.as_contract(&vault, || MockVault::last_group_rel_cap_value(env.clone()));
    assert_eq!(on_vault_value, Some(750_000_000_000_000_000));
}

#[test]
fn group_member_assignment_is_timelocked_and_routes_to_vault() {
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

    let market_id = 5u32;
    let group_id = SdkString::from_str(&env, "group-c");

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_member(
            env.clone(),
            admin.clone(),
            market_id,
            group_id.clone(),
        )
        .unwrap()
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

    let on_vault_market = env.as_contract(&vault, || {
        MockVault::last_group_member_market_id(env.clone())
    });
    assert_eq!(on_vault_market, Some(market_id));
    let on_vault_group = env.as_contract(&vault, || {
        MockVault::last_group_member_group_id(env.clone())
    });
    assert_eq!(on_vault_group, Some(group_id));
}

#[test]
fn group_member_removal_without_existing_membership_is_no_change() {
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

    let market_id = 5u32;
    let empty_group = SdkString::from_str(&env, "");

    let proposal = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_group_member(
            env.clone(),
            admin.clone(),
            market_id,
            empty_group.clone(),
        )
    });

    assert_eq!(proposal, Err(GovernanceError::NoChange));

    let pending = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending.len(), 0);
}

#[test]
fn cap_routes_market_id_and_value_to_vault() {
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

    let market_id = 3u32;
    let cap_value = 10i128;

    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_cap(
            env.clone(),
            admin.clone(),
            market_id,
            cap_value,
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

    let on_vault_market = env.as_contract(&vault, || MockVault::last_cap_market_id(env.clone()));
    assert_eq!(on_vault_market, Some(market_id));
    let on_vault_value = env.as_contract(&vault, || MockVault::last_cap_value(env.clone()));
    assert_eq!(on_vault_value, Some(cap_value));
}

#[test]
fn no_change_returns_error_for_duplicate_submission() {
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

    let sentinel = Address::generate(&env);

    // Set sentinel first
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            sentinel.clone(),
        )
        .unwrap()
    });

    // Submitting the same sentinel again should return NoChange
    let duplicate = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            sentinel.clone(),
        )
    });
    assert_eq!(duplicate, Err(GovernanceError::NoChange));
}

fn submit_initial_fee_config(
    env: &Env,
    governance: &Address,
    admin: &Address,
    performance_recipient: &Address,
    management_recipient: &Address,
) {
    env.as_contract(governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            50_000_000_000_000_000i128,
            performance_recipient.clone(),
            50_000_000_000_000_000i128,
            management_recipient.clone(),
            None,
        )
        .unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: 106,
        protocol_version: 25,
        ..Default::default()
    });

    env.as_contract(governance, || {
        SorobanVaultGovernanceContract::accept_kind(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Fees,
        )
        .unwrap();
    });

    env.ledger().set(LedgerInfo {
        timestamp: 200,
        protocol_version: 25,
        ..Default::default()
    });
}

fn submit_fee_increase(
    env: &Env,
    governance: &Address,
    admin: &Address,
    performance_recipient: &Address,
    management_recipient: &Address,
) -> u64 {
    env.as_contract(governance, || {
        SorobanVaultGovernanceContract::submit_set_fees(
            env.clone(),
            admin.clone(),
            100_000_000_000_000_000i128,
            performance_recipient.clone(),
            50_000_000_000_000_000i128,
            management_recipient.clone(),
            None,
        )
        .unwrap()
    })
}

#[test]
fn revoker_policy_matrix_is_explicit() {
    let cases = [
        (GovernanceActionKind::Pause, true),
        (GovernanceActionKind::Curator, false),
        (GovernanceActionKind::Governance, false),
        (GovernanceActionKind::SupplyQueue, true),
        (GovernanceActionKind::Fees, true),
        (GovernanceActionKind::Restrictions, true),
        (GovernanceActionKind::Sentinel, true),
        (GovernanceActionKind::Cap, true),
        (GovernanceActionKind::MarketRemoval, true),
        (GovernanceActionKind::CapGroup, true),
        (GovernanceActionKind::Skim, false),
        (GovernanceActionKind::TimelockConfig, true),
        (GovernanceActionKind::Other, false),
    ];

    for (kind, sentinel_allowed) in cases {
        assert!(can_revoke_kind(RevokerRole::Admin, kind));
        assert_eq!(
            can_revoke_kind(RevokerRole::Sentinel, kind),
            sentinel_allowed,
            "sentinel revocation policy drift for {:?}",
            kind as u32
        );
    }
}

#[test]
fn sentinel_cannot_revoke_other_pending() {
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
    let sentinel = Address::generate(&env);
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            sentinel.clone(),
        )
        .unwrap();
    });

    let key = Symbol::new(&env, "other_key");
    let payload_hash = BytesN::from_array(&env, &[9u8; 32]);
    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_other(
            env.clone(),
            admin.clone(),
            key.clone(),
            payload_hash.clone(),
        )
        .unwrap()
    });

    let unauthorized = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::revoke_other_pending(
            env.clone(),
            sentinel.clone(),
            key,
            payload_hash,
        )
    });
    assert_eq!(unauthorized, Err(GovernanceError::Unauthorized));
    assert!(env
        .as_contract(&governance, || {
            SorobanVaultGovernanceContract::pending(env.clone(), proposal_id)
        })
        .is_ok());
}

#[test]
fn sentinel_cannot_revoke_governance_proposal_by_id_or_kind() {
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
    let sentinel = Address::generate(&env);
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            sentinel.clone(),
        )
        .unwrap();
    });

    let new_governance = env.register(MockVault, ());
    let proposal_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_governance(
            env.clone(),
            admin.clone(),
            new_governance,
        )
        .unwrap()
    });

    let unauthorized_by_kind = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::revoke_kind(
            env.clone(),
            sentinel.clone(),
            GovernanceActionKind::Governance,
        )
    });
    assert_eq!(unauthorized_by_kind, Err(GovernanceError::Unauthorized));

    let unauthorized_by_id = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::revoke(env.clone(), sentinel.clone(), proposal_id)
    });
    assert_eq!(unauthorized_by_id, Err(GovernanceError::Unauthorized));
    assert!(env
        .as_contract(&governance, || {
            SorobanVaultGovernanceContract::pending(env.clone(), proposal_id)
        })
        .is_ok());
}

#[test]
fn sentinel_revoke_kind_clears_pending() {
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

    // Set first sentinel (immediate)
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            first.clone(),
        )
        .unwrap()
    });

    // Second sentinel change (timelocked)
    env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::submit_set_sentinel(
            env.clone(),
            admin.clone(),
            second.clone(),
        )
        .unwrap()
    });

    let pending_before = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_before.len(), 1);

    // Revoke by kind should clear the pending sentinel change
    let removed = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::revoke_kind(
            env.clone(),
            admin.clone(),
            GovernanceActionKind::Sentinel,
        )
        .unwrap()
    });
    assert_eq!(removed, 1);

    let pending_after = env.as_contract(&governance, || {
        SorobanVaultGovernanceContract::pending_ids(env.clone())
    });
    assert_eq!(pending_after.len(), 0);
}
