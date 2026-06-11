use super::*;
use soroban_sdk::testutils::storage::Instance;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::{Events, Ledger, LedgerInfo};
use soroban_sdk::xdr::{ContractEventBody, ScVal};
use soroban_sdk::{
    address_payload::AddressPayload, contract, contractimpl, symbol_short, BytesN, Env, IntoVal,
    MuxedAddress, Symbol, TryFromVal, Val,
};

#[contract]
struct VaultCaller;

#[contractimpl]
impl VaultCaller {
    fn mint(env: Env, token: Address, to: Address, amount: i128) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "mint"),
            (to, amount).into_val(&env),
        );
    }

    fn burn(env: Env, token: Address, from: Address, amount: i128) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "burn"),
            (from, amount).into_val(&env),
        );
    }

    fn set_paused(env: Env, token: Address, paused: bool) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "set_paused"),
            (env.current_contract_address(), paused).into_val(&env),
        );
    }

    fn set_restrictions(env: Env, token: Address, mode: u32, accounts: soroban_sdk::Vec<Address>) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "set_restrictions"),
            (env.current_contract_address(), mode, accounts).into_val(&env),
        );
    }

    fn approve(
        env: Env,
        token: Address,
        owner: Address,
        spender: Address,
        amount: i128,
        live_until_ledger: u32,
    ) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "approve"),
            (owner, spender, amount, live_until_ledger).into_val(&env),
        );
    }

    fn burn_from(env: Env, token: Address, spender: Address, from: Address, amount: i128) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "burn_from"),
            (spender, from, amount).into_val(&env),
        );
    }

    fn transfer(env: Env, token: Address, from: Address, to: MuxedAddress, amount: i128) {
        env.invoke_contract::<()>(
            &token,
            &soroban_sdk::Symbol::new(&env, "transfer"),
            (from, to, amount).into_val(&env),
        );
    }
}

fn setup() -> (Env, Address, Address, Address) {
    let env = Env::default();
    init_env(&env);

    let vault = env.register(VaultCaller, ());
    let admin = vault.clone();
    let token = env.register(
        SorobanShareTokenContract,
        (
            &admin,
            &vault,
            &String::from_str(&env, "Templar Share"),
            &String::from_str(&env, "tvSHARE"),
            &7u32,
        ),
    );
    (env, admin, vault, token)
}

fn init_env(env: &Env) {
    env.mock_all_auths_allowing_non_root_auth();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        max_entry_ttl: 1_000,
        ..Default::default()
    });
}

fn account_address(env: &Env) -> Address {
    AddressPayload::AccountIdPublicKeyEd25519(BytesN::from_array(env, &[7; 32])).to_address(env)
}

#[test]
#[should_panic]
fn constructor_rejects_account_vault_address() {
    let env = Env::default();
    init_env(&env);

    let admin = Address::generate(&env);
    let account_vault = account_address(&env);
    env.register(
        SorobanShareTokenContract,
        (
            &admin,
            &account_vault,
            &String::from_str(&env, "Templar Share"),
            &String::from_str(&env, "tvSHARE"),
            &7u32,
        ),
    );
}

#[test]
fn constructor_emits_config_event() {
    let (env, admin, vault, token) = setup();
    let filtered_events = env.events().all().filter_by_contract(&token);
    let events = filtered_events.events();
    let event = events.last().expect("constructor config event");
    let ContractEventBody::V0(body) = &event.body;
    assert_eq!(body.topics.len(), 3);
    assert_eq!(
        body.topics[0],
        ScVal::try_from_val(&env, &symbol_short!("config")).unwrap()
    );
    let admin_val: Val = admin.into_val(&env);
    let vault_val: Val = vault.into_val(&env);
    assert_eq!(
        body.topics[1],
        ScVal::try_from_val(&env, &admin_val).unwrap()
    );
    assert_eq!(
        body.topics[2],
        ScVal::try_from_val(&env, &vault_val).unwrap()
    );
}

#[test]
fn set_vault_rejects_account_address() {
    let (env, admin, vault, token) = setup();
    let account_vault = account_address(&env);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "set_vault"),
        (&admin, &account_vault).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::InvalidInput)));

    let configured_vault: Address =
        env.invoke_contract(&token, &Symbol::new(&env, "vault"), ().into_val(&env));
    assert_eq!(configured_vault, vault);
}

#[test]
fn constructor_rejects_external_share_token_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let external_admin = Address::generate(&env);
    let vault = env.register(VaultCaller, ());
    let token = Address::generate(&env);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.register_at(
            &token,
            SorobanShareTokenContract,
            (
                &external_admin,
                &vault,
                &String::from_str(&env, "Templar Share"),
                &String::from_str(&env, "tvSHARE"),
                &7u32,
            ),
        );
    }));

    assert!(result.is_err());
}

#[test]
fn set_admin_rejects_non_vault_admin() {
    let (env, admin, _vault, token) = setup();
    let new_admin = Address::generate(&env);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &new_admin).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::InvalidInput)));
}

#[test]
fn vault_can_mint() {
    let (env, _admin, vault, token) = setup();
    let user = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
    });

    let bal: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&user,).into_val(&env),
    );
    assert_eq!(bal, 1000);
}

#[test]
fn vault_can_burn() {
    let (env, _admin, vault, token) = setup();
    let user = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
    });
    env.as_contract(&vault, || {
        VaultCaller::burn(env.clone(), token.clone(), user.clone(), 400);
    });

    let bal: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&user,).into_val(&env),
    );
    assert_eq!(bal, 600);
}

#[test]
fn burn_from_emits_supplemental_spender_event() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
    });
    env.as_contract(&vault, || {
        VaultCaller::approve(
            env.clone(),
            token.clone(),
            from.clone(),
            spender.clone(),
            400,
            300,
        );
    });
    let event_count_before_burn_from = env.events().all().filter_by_contract(&token).events().len();

    env.as_contract(&vault, || {
        VaultCaller::burn_from(
            env.clone(),
            token.clone(),
            spender.clone(),
            from.clone(),
            250,
        );
    });

    let events = env.events().all().filter_by_contract(&token);
    let new_events = &events.events()[event_count_before_burn_from..];
    let burn_from_event = new_events
        .iter()
        .find(|event| {
            let ContractEventBody::V0(body) = &event.body;
            body.topics.first()
                == Some(&ScVal::try_from_val(&env, &symbol_short!("burn_from")).unwrap())
        })
        .expect("burn_from event must be emitted");
    let ContractEventBody::V0(body) = &burn_from_event.body;
    assert_eq!(body.topics.len(), 3);
    assert_eq!(
        body.topics[0],
        ScVal::try_from_val(&env, &symbol_short!("burn_from")).unwrap()
    );
    let spender_val: Val = spender.clone().into_val(&env);
    let from_val: Val = from.clone().into_val(&env);
    let amount_val: Val = 250i128.into_val(&env);
    assert_eq!(
        body.topics[1],
        ScVal::try_from_val(&env, &spender_val).unwrap()
    );
    assert_eq!(
        body.topics[2],
        ScVal::try_from_val(&env, &from_val).unwrap()
    );
    assert_eq!(body.data, ScVal::try_from_val(&env, &amount_val).unwrap());

    let bal: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&from,).into_val(&env),
    );
    let allowance: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "allowance"),
        (&from, &spender).into_val(&env),
    );
    assert_eq!(bal, 750);
    assert_eq!(allowance, 150);
}

#[test]
fn direct_burn_is_vault_only_protocol_effect() {
    let (env, _admin, vault, token) = setup();
    let user = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
    });

    // A direct call without vault authorization remains rejected because burn is not
    // a public owner API; it is a vault-only protocol settlement primitive.
    env.mock_auths(&[]);
    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "burn"),
        (&user, &1i128).into_val(&env),
    );
    assert!(err.is_err());

    // The configured vault can execute the burn without re-requiring owner auth here.
    // Owner consent is enforced by vault entrypoints before generating BurnShares effects.
    init_env(&env);
    env.as_contract(&vault, || {
        VaultCaller::burn(env.clone(), token.clone(), user.clone(), 400);
    });

    let bal: i128 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "balance"),
        (&user,).into_val(&env),
    );
    assert_eq!(bal, 600);
}

#[test]
fn admin_can_pause_and_unpause_share_token_transfers() {
    let (env, admin, vault, token) = setup();
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
    });

    env.invoke_contract::<()>(
        &token,
        &Symbol::new(&env, "set_paused"),
        (&admin, &true).into_val(&env),
    );
    assert!(env.invoke_contract::<bool>(&token, &Symbol::new(&env, "paused"), ().into_val(&env),));

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "transfer"),
        (&from, MuxedAddress::from(to.clone()), &1i128).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Paused)));

    env.invoke_contract::<()>(
        &token,
        &Symbol::new(&env, "set_paused"),
        (&admin, &false).into_val(&env),
    );
    env.invoke_contract::<()>(
        &token,
        &Symbol::new(&env, "transfer"),
        (&from, MuxedAddress::from(to.clone()), &250i128).into_val(&env),
    );

    let to_bal: i128 =
        env.invoke_contract(&token, &Symbol::new(&env, "balance"), (&to,).into_val(&env));
    assert_eq!(to_bal, 250);
}

#[test]
fn vault_can_pause_share_token_mint_and_burn() {
    let (env, _admin, vault, token) = setup();
    let user = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::set_paused(env.clone(), token.clone(), true);
        let err = env.try_invoke_contract::<(), ShareTokenError>(
            &token,
            &Symbol::new(&env, "mint"),
            (&user, &100i128).into_val(&env),
        );
        assert_eq!(err, Err(Ok(ShareTokenError::Paused)));
    });

    env.as_contract(&vault, || {
        VaultCaller::set_paused(env.clone(), token.clone(), false);
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
        VaultCaller::set_paused(env.clone(), token.clone(), true);
        let err = env.try_invoke_contract::<(), ShareTokenError>(
            &token,
            &Symbol::new(&env, "burn"),
            (&user, &100i128).into_val(&env),
        );
        assert_eq!(err, Err(Ok(ShareTokenError::Paused)));
    });
}

#[test]
fn share_token_restrictions_block_blacklisted_sender_and_recipient() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let blocked = Address::generate(&env);
    let allowed = Address::generate(&env);
    let mut accounts = soroban_sdk::Vec::new(&env);
    accounts.push_back(blocked.clone());

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
        VaultCaller::mint(env.clone(), token.clone(), blocked.clone(), 1000);
        VaultCaller::set_restrictions(env.clone(), token.clone(), 1, accounts.clone());
    });

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "transfer"),
        (&from, MuxedAddress::from(blocked.clone()), &1i128).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Restricted)));

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "transfer"),
        (&blocked, MuxedAddress::from(allowed.clone()), &1i128).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Restricted)));
}

#[test]
fn share_token_whitelist_allows_only_listed_transfer_parties() {
    let (env, _admin, vault, token) = setup();
    let listed_from = Address::generate(&env);
    let listed_to = Address::generate(&env);
    let unlisted = Address::generate(&env);
    let mut accounts = soroban_sdk::Vec::new(&env);
    accounts.push_back(listed_from.clone());
    accounts.push_back(listed_to.clone());

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), listed_from.clone(), 1000);
        VaultCaller::set_restrictions(env.clone(), token.clone(), 2, accounts.clone());
    });

    env.invoke_contract::<()>(
        &token,
        &Symbol::new(&env, "transfer"),
        (
            &listed_from,
            MuxedAddress::from(listed_to.clone()),
            &250i128,
        )
            .into_val(&env),
    );
    let listed_to_bal: i128 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "balance"),
        (&listed_to,).into_val(&env),
    );
    assert_eq!(listed_to_bal, 250);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "transfer"),
        (&listed_from, MuxedAddress::from(unlisted.clone()), &1i128).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Restricted)));
}

#[test]
fn share_token_whitelist_allows_vault_authorized_escrow_to_vault() {
    let (env, _admin, vault, token) = setup();
    let listed_owner = Address::generate(&env);
    let unlisted = Address::generate(&env);
    let mut accounts = soroban_sdk::Vec::new(&env);
    accounts.push_back(listed_owner.clone());

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), listed_owner.clone(), 1000);
        VaultCaller::set_restrictions(env.clone(), token.clone(), 2, accounts.clone());
        VaultCaller::transfer(
            env.clone(),
            token.clone(),
            listed_owner.clone(),
            MuxedAddress::from(vault.clone()),
            250,
        );
        VaultCaller::transfer(
            env.clone(),
            token.clone(),
            vault.clone(),
            MuxedAddress::from(listed_owner.clone()),
            100,
        );
    });

    let vault_balance: i128 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "balance"),
        (&vault,).into_val(&env),
    );
    assert_eq!(vault_balance, 150);
    let owner_balance: i128 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "balance"),
        (&listed_owner,).into_val(&env),
    );
    assert_eq!(owner_balance, 850);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "transfer"),
        (&listed_owner, MuxedAddress::from(unlisted.clone()), &1i128).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Restricted)));
}

#[test]
fn set_admin_rejects_admin_rotation_away_from_vault() {
    let (env, admin, vault, token) = setup();
    let new_admin = Address::generate(&env);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &new_admin).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::InvalidInput)));

    let stored_admin: Address = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "admin"),
        ().into_val(&env),
    );
    assert_eq!(stored_admin, vault);
    let pending_admin: Option<Address> = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "pending_admin"),
        ().into_val(&env),
    );
    assert_eq!(pending_admin, None);
}

#[test]
fn set_admin_emits_propose_and_accept_events() {
    let (env, admin, _vault, token) = setup();
    let retained_admin = admin.clone();

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &retained_admin).into_val(&env),
    );
    let filtered_events = env.events().all().filter_by_contract(&token);
    let events = filtered_events.events();
    let admin_set = ScVal::try_from_val(&env, &symbol_short!("admin_set")).unwrap();
    assert!(events.iter().any(|event| {
        let ContractEventBody::V0(body) = &event.body;
        body.topics.first() == Some(&admin_set)
    }));

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "accept_admin"),
        (&retained_admin,).into_val(&env),
    );
    let filtered_events = env.events().all().filter_by_contract(&token);
    let events = filtered_events.events();
    let admin_acc = ScVal::try_from_val(&env, &symbol_short!("admin_acc")).unwrap();
    assert!(events.iter().any(|event| {
        let ContractEventBody::V0(body) = &event.body;
        body.topics.first() == Some(&admin_acc)
    }));
}

#[test]
fn non_admin_cannot_set_admin() {
    let (env, _admin, _vault, token) = setup();
    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&non_admin, &new_admin).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Unauthorized)));
}

#[test]
fn failed_admin_rotation_leaves_vault_admin_authorized() {
    let (env, admin, _vault, token) = setup();
    let new_admin = Address::generate(&env);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &new_admin).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::InvalidInput)));

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_paused"),
        (&admin, &true).into_val(&env),
    );
    assert!(env.invoke_contract::<bool>(&token, &Symbol::new(&env, "paused"), ().into_val(&env)));
}

#[test]
fn share_token_upgrade_requires_admin_and_emits_event() {
    let (env, admin, _vault, token) = setup();
    env.cost_estimate().budget().reset_unlimited();
    let new_hash = empty_wasm_hash(&env);
    env.as_contract(&token, || {
        SorobanShareTokenContract::upgrade(&env, new_hash.clone(), admin.clone());
    });
    let filtered_events = env.events().all().filter_by_contract(&token);
    let events = filtered_events.events();
    let upgrade = ScVal::try_from_val(&env, &symbol_short!("upgrade")).unwrap();
    assert!(events.iter().any(|event| {
        let ContractEventBody::V0(body) = &event.body;
        body.topics.first() == Some(&upgrade)
    }));
}

#[test]
fn burn_from_without_allowance_fails() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
        let err = env.try_invoke_contract::<(), ShareTokenError>(
            &token,
            &soroban_sdk::Symbol::new(&env, "burn_from"),
            (&spender, &from, &1i128).into_val(&env),
        );
        assert!(err.is_err());
    });
}

#[test]
fn burn_from_over_allowance_fails() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
        VaultCaller::approve(
            env.clone(),
            token.clone(),
            from.clone(),
            spender.clone(),
            100,
            300,
        );
        let err = env.try_invoke_contract::<(), ShareTokenError>(
            &token,
            &soroban_sdk::Symbol::new(&env, "burn_from"),
            (&spender, &from, &101i128).into_val(&env),
        );
        assert!(err.is_err());
    });
}

#[test]
fn burn_from_after_allowance_expiry_fails() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
        VaultCaller::approve(
            env.clone(),
            token.clone(),
            from.clone(),
            spender.clone(),
            100,
            101,
        );
    });
    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 102,
        max_entry_ttl: 1_000,
        ..Default::default()
    });

    env.as_contract(&vault, || {
        let err = env.try_invoke_contract::<(), ShareTokenError>(
            &token,
            &soroban_sdk::Symbol::new(&env, "burn_from"),
            (&spender, &from, &1i128).into_val(&env),
        );
        assert!(err.is_err());
    });
}

#[test]
fn direct_caller_cannot_burn_from_even_with_allowance() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
        VaultCaller::approve(
            env.clone(),
            token.clone(),
            from.clone(),
            spender.clone(),
            100,
            300,
        );
    });

    env.mock_auths(&[]);
    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &soroban_sdk::Symbol::new(&env, "burn_from"),
        (&spender, &from, &1i128).into_val(&env),
    );
    assert!(err.is_err());
}

#[test]
fn user_can_transfer_with_auth() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
    });

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "transfer"),
        (&from, MuxedAddress::from(to.clone()), &250i128).into_val(&env),
    );

    let from_bal: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&from,).into_val(&env),
    );
    let to_bal: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&to,).into_val(&env),
    );
    assert_eq!(from_bal, 750);
    assert_eq!(to_bal, 250);
}

#[test]
fn transfer_without_from_auth_fails() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    // Mint some tokens first so the failure is about auth, not balance
    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
    });

    // Don't mock auths — this should fail on from.require_auth()
    env.mock_auths(&[]);
    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &soroban_sdk::Symbol::new(&env, "transfer"),
        (&from, MuxedAddress::from(to), &1i128).into_val(&env),
    );
    assert!(err.is_err());
}

fn empty_wasm_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(
        env,
        &[
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ],
    )
}

#[test]
fn metadata_returns_constructor_values() {
    let (env, _admin, _vault, token) = setup();

    let name: String = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "name"),
        ().into_val(&env),
    );
    let symbol: String = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "symbol"),
        ().into_val(&env),
    );
    let decimals: u32 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "decimals"),
        ().into_val(&env),
    );

    assert_eq!(name, String::from_str(&env, "Templar Share"));
    assert_eq!(symbol, String::from_str(&env, "tvSHARE"));
    assert_eq!(decimals, 7);
}

#[test]
fn admin_cannot_change_metadata_after_deployment() {
    let (env, admin, _vault, token) = setup();

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "set_metadata"),
        (
            &admin,
            &String::from_str(&env, "Mutable Share"),
            &String::from_str(&env, "MUT"),
            &18u32,
        )
            .into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::MetadataImmutable)));

    let name: String = env.invoke_contract(&token, &Symbol::new(&env, "name"), ().into_val(&env));
    let symbol: String =
        env.invoke_contract(&token, &Symbol::new(&env, "symbol"), ().into_val(&env));
    let decimals: u32 =
        env.invoke_contract(&token, &Symbol::new(&env, "decimals"), ().into_val(&env));

    assert_eq!(name, String::from_str(&env, "Templar Share"));
    assert_eq!(symbol, String::from_str(&env, "tvSHARE"));
    assert_eq!(decimals, 7);
}

#[test]
fn read_only_entrypoints_cover_share_token_ttl_maintenance_surface() {
    let (env, _admin, vault, token) = setup();
    let user = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
        VaultCaller::approve(
            env.clone(),
            token.clone(),
            user.clone(),
            spender.clone(),
            250,
            300,
        );
    });

    let supply: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "total_supply"),
        ().into_val(&env),
    );
    let balance: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&user,).into_val(&env),
    );
    let allowance: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "allowance"),
        (&user, &spender).into_val(&env),
    );
    let name: String = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "name"),
        ().into_val(&env),
    );
    let symbol: String = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "symbol"),
        ().into_val(&env),
    );
    let decimals: u32 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "decimals"),
        ().into_val(&env),
    );

    assert_eq!(supply, 1000);
    assert_eq!(balance, 1000);
    assert_eq!(allowance, 250);
    assert_eq!(name, String::from_str(&env, "Templar Share"));
    assert_eq!(symbol, String::from_str(&env, "tvSHARE"));
    assert_eq!(decimals, 7);

    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 2_592_100,
        max_entry_ttl: 3_110_400,
        ..Default::default()
    });
    let ttl_before_read = env.as_contract(&token, || env.storage().instance().get_ttl());
    let refreshed_supply: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "total_supply"),
        ().into_val(&env),
    );
    let ttl_after_read = env.as_contract(&token, || env.storage().instance().get_ttl());

    assert_eq!(refreshed_supply, 1000);
    assert!(ttl_after_read > ttl_before_read);
}

#[test]
fn admin_extend_ttl_preserves_holder_balances_and_allowance_expiry_semantics() {
    let (env, admin, vault, token) = setup();
    let user = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
        VaultCaller::approve(
            env.clone(),
            token.clone(),
            user.clone(),
            spender.clone(),
            400,
            150,
        );
    });

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "extend_ttl"),
        (&admin,).into_val(&env),
    );

    let balance: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "balance"),
        (&user,).into_val(&env),
    );
    let allowance_before_expiry: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "allowance"),
        (&user, &spender).into_val(&env),
    );
    assert_eq!(balance, 1000);
    assert_eq!(allowance_before_expiry, 400);

    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 151,
        max_entry_ttl: 1_000,
        ..Default::default()
    });

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "extend_ttl"),
        (&admin,).into_val(&env),
    );
    let allowance_after_expiry: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "allowance"),
        (&user, &spender).into_val(&env),
    );
    assert_eq!(allowance_after_expiry, 0);
}

#[test]
fn total_supply_tracks_mint_and_burn() {
    let (env, _admin, vault, token) = setup();
    let user = Address::generate(&env);

    let supply: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "total_supply"),
        ().into_val(&env),
    );
    assert_eq!(supply, 0);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 500);
    });

    let supply: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "total_supply"),
        ().into_val(&env),
    );
    assert_eq!(supply, 500);

    env.as_contract(&vault, || {
        VaultCaller::burn(env.clone(), token.clone(), user.clone(), 200);
    });

    let supply: i128 = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "total_supply"),
        ().into_val(&env),
    );
    assert_eq!(supply, 300);
}

#[test]
fn admin_cannot_rebind_vault_after_init() {
    let (env, admin, vault, token) = setup();
    let replacement_vault = env.register(VaultCaller, ());

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "set_vault"),
        (&admin, &replacement_vault).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::VaultImmutable)));

    let configured_vault: Address =
        env.invoke_contract(&token, &Symbol::new(&env, "vault"), ().into_val(&env));
    assert_eq!(configured_vault, vault);
}

#[test]
fn non_admin_cannot_rebind_vault() {
    let (env, _admin, vault, token) = setup();
    let attacker = Address::generate(&env);
    let replacement_vault = env.register(VaultCaller, ());

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "set_vault"),
        (&attacker, &replacement_vault).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::Unauthorized)));

    let configured_vault: Address =
        env.invoke_contract(&token, &Symbol::new(&env, "vault"), ().into_val(&env));
    assert_eq!(configured_vault, vault);
}

#[test]
fn original_vault_keeps_mint_burn_authority_after_failed_rebind() {
    let (env, admin, vault, token) = setup();
    let replacement_vault = env.register(VaultCaller, ());
    let user = Address::generate(&env);

    let err = env.try_invoke_contract::<(), ShareTokenError>(
        &token,
        &Symbol::new(&env, "set_vault"),
        (&admin, &replacement_vault).into_val(&env),
    );
    assert_eq!(err, Err(Ok(ShareTokenError::VaultImmutable)));

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 500);
    });
    env.as_contract(&vault, || {
        VaultCaller::burn(env.clone(), token.clone(), user.clone(), 125);
    });

    let balance: i128 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "balance"),
        (&user,).into_val(&env),
    );
    assert_eq!(balance, 375);
}
