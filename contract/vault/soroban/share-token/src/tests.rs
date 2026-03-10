use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::{Ledger, LedgerInfo};
use soroban_sdk::{contract, contractimpl, IntoVal};

#[contract]
struct VaultCaller;

#[contractimpl]
impl VaultCaller {
    fn mint(env: Env, token: Address, to: Address, amount: i128) {
        env.invoke_contract::<Result<(), ShareTokenError>>(
            &token,
            &soroban_sdk::Symbol::new(&env, "mint"),
            (to, amount).into_val(&env),
        )
        .unwrap();
    }
}

#[test]
fn vault_can_mint() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
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
    let user = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), user.clone(), 1000);
    });

    let bal = env.as_contract(&token, || {
        SorobanShareTokenContract::balance(env.clone(), user)
    });
    assert_eq!(bal, 1000);
}

#[test]
fn user_can_transfer_with_auth() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
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

    let from = Address::generate(&env);
    let to = Address::generate(&env);

    env.as_contract(&vault, || {
        VaultCaller::mint(env.clone(), token.clone(), from.clone(), 1000);
    });

    env.as_contract(&token, || {
        SorobanShareTokenContract::transfer(env.clone(), from.clone(), to.clone(), 250).unwrap();
    });

    let from_bal = env.as_contract(&token, || {
        SorobanShareTokenContract::balance(env.clone(), from)
    });
    let to_bal = env.as_contract(&token, || {
        SorobanShareTokenContract::balance(env.clone(), to)
    });
    assert_eq!(from_bal, 750);
    assert_eq!(to_bal, 250);
}

#[test]
#[should_panic]
fn transfer_without_from_auth_panics() {
    let env = Env::default();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 23,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let vault = Address::generate(&env);
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

    let user = Address::generate(&env);
    env.as_contract(&token, || {
        let _ = SorobanShareTokenContract::transfer(env.clone(), user.clone(), admin.clone(), 1);
    });
}
