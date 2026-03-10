use blend_contract_sdk::{
    pool,
    testutils::{default_reserve_config, BlendFixture},
};
use soroban_sdk::{
    testutils::{Address as _, BytesN as _},
    token::StellarAssetClient,
    Address, BytesN, Env, String,
};
use templar_soroban_blend_adapter::BlendAdapterContract;
use templar_soroban_runtime::contract::SorobanVaultContract;

fn setup_blend_pool(
    env: &Env,
) -> (
    Address,
    pool::Client<'_>,
    Address,
    StellarAssetClient<'_>,
    Address,
) {
    let deployer = Address::generate(env);
    let blnd = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let usdc = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let blend = BlendFixture::deploy(env, &deployer, &blnd, &usdc);

    let asset_sac = env.register_stellar_asset_contract_v2(deployer.clone());
    let asset = asset_sac.address();
    let asset_admin = StellarAssetClient::new(env, &asset);

    let pool_addr = blend.pool_factory.mock_all_auths().deploy(
        &deployer,
        &String::from_str(env, "templar"),
        &BytesN::<32>::random(env),
        &Address::generate(env),
        &0_1000000,
        &4,
        &1_0000000,
    );
    let pool_client = pool::Client::new(env, &pool_addr);

    let reserve_config = default_reserve_config();
    pool_client
        .mock_all_auths()
        .queue_set_reserve(&asset, &reserve_config);
    pool_client.mock_all_auths().set_reserve(&asset);

    blend
        .backstop
        .mock_all_auths()
        .deposit(&deployer, &pool_addr, &50_000_0000000);
    pool_client.mock_all_auths().set_status(&3);
    pool_client.mock_all_auths().update_status();

    (pool_addr, pool_client, asset, asset_admin, deployer)
}

fn vault_snapshot(env: &Env, vault: &Address) -> (i128, i128, i128) {
    env.as_contract(vault, || {
        SorobanVaultContract::vault_snapshot(env.clone()).unwrap()
    })
}

#[test]
fn vault_allocates_supply_to_blend_and_withdraws_back() {
    let env = Env::default();
    env.mock_all_auths();

    let governance = Address::generate(&env);
    let allocator = Address::generate(&env);
    let user = Address::generate(&env);
    let vault = env.register(SorobanVaultContract, ());

    let (pool, pool_client, asset, asset_admin, _deployer) = setup_blend_pool(&env);
    let share = env
        .register_stellar_asset_contract_v2(vault.clone())
        .address();
    let adapter_admin = Address::generate(&env);
    let adapter = env.register(BlendAdapterContract, (&adapter_admin, &vault, &pool));
    let asset_client = soroban_sdk::token::Client::new(&env, &asset);

    env.as_contract(&vault, || {
        SorobanVaultContract::initialize(
            env.clone(),
            governance.clone(),
            governance.clone(),
            asset.clone(),
            share,
        )
        .unwrap();
    });
    env.as_contract(&vault, || {
        SorobanVaultContract::set_allocators(
            env.clone(),
            governance.clone(),
            soroban_sdk::Vec::from_array(&env, [allocator.clone()]),
        )
        .unwrap();
    });
    env.as_contract(&vault, || {
        SorobanVaultContract::set_supply_queue(
            env.clone(),
            governance.clone(),
            soroban_sdk::Vec::from_array(&env, [0u32]),
        )
        .unwrap();
    });
    env.as_contract(&vault, || {
        SorobanVaultContract::set_allowed_adapters(
            env.clone(),
            governance.clone(),
            soroban_sdk::Vec::from_array(&env, [adapter.clone()]),
        )
        .unwrap();
    });

    let deposit_amount = 1_000_0000000;
    let supply_amount = 600_0000000;
    let withdraw_amount = 250_0000000;

    asset_admin.mint(&user, &deposit_amount);

    let minted = env
        .as_contract(&vault, || {
            SorobanVaultContract::deposit_with_min(
                env.clone(),
                user.clone(),
                user.clone(),
                deposit_amount,
                0,
            )
        })
        .unwrap();
    assert_eq!(minted, deposit_amount);
    assert_eq!(
        vault_snapshot(&env, &vault),
        (deposit_amount, deposit_amount, 0)
    );

    let new_external = env
        .as_contract(&vault, || {
            SorobanVaultContract::allocate_supply(env.clone(), allocator.clone(), 0, supply_amount)
        })
        .unwrap();
    assert_eq!(new_external, supply_amount);
    assert_eq!(
        vault_snapshot(&env, &vault),
        (
            deposit_amount,
            deposit_amount - supply_amount,
            supply_amount
        )
    );
    assert_eq!(asset_client.balance(&vault), deposit_amount - supply_amount);

    let positions_after_supply = pool_client.get_positions(&adapter);
    let b_tokens_after_supply = positions_after_supply.supply.get(0).unwrap_or(0);
    assert!(b_tokens_after_supply > 0);

    let refreshed_external = env
        .as_contract(&vault, || {
            SorobanVaultContract::refresh_markets(
                env.clone(),
                allocator.clone(),
                soroban_sdk::Vec::from_array(&env, [0u32]),
            )
        })
        .unwrap();
    assert_eq!(refreshed_external, supply_amount);
    assert_eq!(
        vault_snapshot(&env, &vault),
        (
            deposit_amount,
            deposit_amount - supply_amount,
            supply_amount
        )
    );

    let remaining_external = env
        .as_contract(&vault, || {
            SorobanVaultContract::allocate_withdraw(
                env.clone(),
                allocator.clone(),
                0,
                withdraw_amount,
            )
        })
        .unwrap();
    assert_eq!(remaining_external, supply_amount - withdraw_amount);
    assert_eq!(
        vault_snapshot(&env, &vault),
        (
            deposit_amount,
            deposit_amount - supply_amount + withdraw_amount,
            supply_amount - withdraw_amount,
        )
    );
    assert_eq!(
        asset_client.balance(&vault),
        deposit_amount - supply_amount + withdraw_amount
    );

    let positions_after_withdraw = pool_client.get_positions(&adapter);
    let b_tokens_after_withdraw = positions_after_withdraw.supply.get(0).unwrap_or(0);
    assert!(b_tokens_after_withdraw > 0);
    assert!(b_tokens_after_withdraw < b_tokens_after_supply);
}
