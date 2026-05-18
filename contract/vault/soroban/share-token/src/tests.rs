use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::{Ledger, LedgerInfo};
use soroban_sdk::{contract, contractimpl, IntoVal, MuxedAddress, Symbol};

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
}

fn setup() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });

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
