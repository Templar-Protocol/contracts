use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::{Events, Ledger, LedgerInfo};
use soroban_sdk::xdr::{ContractEventBody, ScVal};
use soroban_sdk::{
    address_payload::AddressPayload, contract, contractimpl, symbol_short, BytesN, IntoVal,
    MuxedAddress, TryFromVal, Val,
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
}

fn setup() -> (Env, Address, Address, Address) {
    let env = Env::default();
    init_env(&env);

    let admin = Address::generate(&env);
    let vault = env.register(VaultCaller, ());
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
#[should_panic]
fn set_vault_rejects_account_address() {
    let (env, admin, _vault, token) = setup();
    let account_vault = account_address(&env);

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_vault"),
        (&admin, &account_vault).into_val(&env),
    );
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
    env.events().all();

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
    assert_eq!(events.events().len(), 2);
    let burn_from_event = &events.events()[1];
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
fn set_admin_rotates_admin() {
    let (env, admin, _vault, token) = setup();
    let new_admin = Address::generate(&env);

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &new_admin).into_val(&env),
    );

    let stored_admin: Address = env.invoke_contract(
        &token,
        &soroban_sdk::Symbol::new(&env, "admin"),
        ().into_val(&env),
    );
    assert_eq!(stored_admin, new_admin);
}

#[test]
#[should_panic]
fn non_admin_cannot_set_admin() {
    let (env, _admin, _vault, token) = setup();
    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&non_admin, &new_admin).into_val(&env),
    );
}

#[test]
#[should_panic]
fn old_admin_loses_privilege_after_rotation() {
    let (env, admin, _vault, token) = setup();
    let new_admin = Address::generate(&env);

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &new_admin).into_val(&env),
    );

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_admin"),
        (&admin, &Address::generate(&env)).into_val(&env),
    );
}

#[test]
#[should_panic]
fn burn_from_without_allowance_panics() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
        VaultCaller::burn_from(env.clone(), token.clone(), spender.clone(), from.clone(), 1);
    });
}

#[test]
#[should_panic]
fn burn_from_over_allowance_panics() {
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
        VaultCaller::burn_from(
            env.clone(),
            token.clone(),
            spender.clone(),
            from.clone(),
            101,
        );
    });
}

#[test]
#[should_panic]
fn burn_from_after_allowance_expiry_panics() {
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
        VaultCaller::burn_from(env.clone(), token.clone(), spender.clone(), from.clone(), 1);
    });
}

#[test]
#[should_panic]
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
    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "burn_from"),
        (&spender, &from, &1i128).into_val(&env),
    );
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
#[should_panic]
fn transfer_without_from_auth_panics() {
    let (env, _admin, vault, token) = setup();
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    // Mint some tokens first so the failure is about auth, not balance
    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
    });

    // Don't mock auths — this should panic on from.require_auth()
    env.mock_auths(&[]);
    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "transfer"),
        (&from, MuxedAddress::from(to), &1i128).into_val(&env),
    );
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
#[should_panic]
fn admin_cannot_change_metadata_after_deployment() {
    let (env, admin, _vault, token) = setup();

    env.invoke_contract::<()>(
        &token,
        &soroban_sdk::Symbol::new(&env, "set_metadata"),
        (
            &admin,
            &String::from_str(&env, "Mutable Share"),
            &String::from_str(&env, "MUT"),
            &18u32,
        )
            .into_val(&env),
    );
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
