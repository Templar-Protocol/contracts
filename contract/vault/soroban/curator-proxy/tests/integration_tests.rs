use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol,
};
use templar_curator_proxy_soroban::{ContractError, SorobanCuratorProxyContract};
use templar_soroban_governance::SorobanVaultGovernanceContract;
use templar_soroban_runtime::{SorobanVaultContract, VaultDataKey};

struct Harness {
    env: Env,
    admin: Address,
    vault: Address,
    governance: Address,
    proxy: Address,
}

fn setup_harness() -> Harness {
    setup_harness_with_auth_mock(true)
}

fn setup_harness_no_auth_mock() -> Harness {
    setup_harness_with_auth_mock(false)
}

fn setup_harness_with_auth_mock(mock_auth: bool) -> Harness {
    let env = Env::default();
    if mock_auth {
        env.mock_all_auths_allowing_non_root_auth();
    }
    let admin = Address::generate(&env);
    let vault = env.register(SorobanVaultContract, ());
    let governance = env.register(SorobanVaultGovernanceContract, (&admin, &vault, &0u64));
    let asset_admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(asset_admin)
        .address();
    let share = env
        .register_stellar_asset_contract_v2(vault.clone())
        .address();

    env.invoke_contract::<()>(
        &vault,
        &Symbol::new(&env, "initialize"),
        (&governance, &governance, &asset, &share, &0i128, &0i128).into_val(&env),
    );

    let proxy = env.register(SorobanCuratorProxyContract, ());
    env.invoke_contract::<()>(
        &proxy,
        &Symbol::new(&env, "initialize"),
        (&vault, &governance).into_val(&env),
    );

    Harness {
        env,
        admin,
        vault,
        governance,
        proxy,
    }
}

#[test]
fn proxy_submits_sentinel_change_through_real_governance_contract() {
    let harness = setup_harness();

    let proposal_id = harness.env.invoke_contract::<u64>(
        &harness.proxy,
        &Symbol::new(&harness.env, "set_sentinel"),
        (&harness.admin, &harness.admin).into_val(&harness.env),
    );

    assert_eq!(proposal_id, 1);
    assert_eq!(
        harness.env.invoke_contract::<Option<Address>>(
            &harness.proxy,
            &Symbol::new(&harness.env, "sentinel"),
            soroban_sdk::vec![&harness.env],
        ),
        Some(harness.admin.clone())
    );
    harness.env.as_contract(&harness.vault, || {
        assert_eq!(
            harness
                .env
                .storage()
                .instance()
                .get(&VaultDataKey::Sentinel),
            Some(harness.admin.clone())
        );
    });
}

#[test]
fn proxy_exposes_governance_views() {
    let harness = setup_harness();

    let governance = harness.env.invoke_contract::<Address>(
        &harness.proxy,
        &Symbol::new(&harness.env, "governance"),
        soroban_sdk::vec![&harness.env],
    );
    let admin = harness.env.invoke_contract::<Address>(
        &harness.proxy,
        &Symbol::new(&harness.env, "admin"),
        soroban_sdk::vec![&harness.env],
    );

    assert_eq!(governance, harness.governance);
    assert_eq!(admin, harness.admin);
}

#[test]
fn proxy_set_sentinel_rejected_for_unauthorized() {
    let harness = setup_harness_no_auth_mock();
    let attacker = Address::generate(&harness.env);
    let sentinel = Address::generate(&harness.env);
    let governance_auth = MockAuthInvoke {
        contract: &harness.governance,
        fn_name: "submit_set_sentinel",
        args: (&attacker, &sentinel).into_val(&harness.env),
        sub_invokes: &[],
    };
    let proxy_auth = MockAuthInvoke {
        contract: &harness.proxy,
        fn_name: "set_sentinel",
        args: (&attacker, &sentinel).into_val(&harness.env),
        sub_invokes: &[governance_auth],
    };
    harness.env.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &proxy_auth,
    }]);

    let result = harness.env.try_invoke_contract::<u64, ContractError>(
        &harness.proxy,
        &Symbol::new(&harness.env, "set_sentinel"),
        (&attacker, &sentinel).into_val(&harness.env),
    );

    assert_eq!(result, Err(Ok(ContractError::GovernanceError)));
}
