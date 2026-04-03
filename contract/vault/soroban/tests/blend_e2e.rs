use blend_contract_sdk::{
    pool,
    testutils::{default_reserve_config, BlendFixture},
};
use soroban_sdk::{
    testutils::{Address as _, BytesN as _},
    token::StellarAssetClient,
    Address, Bytes, BytesN, Env, String,
};
use std::string::String as StdString;
use templar_soroban_blend_adapter::BlendAdapterContract;
use templar_soroban_runtime::contract::SorobanVaultContract;
use templar_soroban_shared_types::{
    VaultCommand, VaultCommandResult, GOVERNANCE_CONFIG_KIND_ALLOCATORS,
    GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS, GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};

fn address_text(address: &Address) -> StdString {
    StdString::from_utf8(address.to_string().to_bytes().to_alloc_vec()).unwrap()
}

fn execute_command(
    env: &Env,
    command: &VaultCommand,
) -> Result<VaultCommandResult, templar_soroban_runtime::ContractError> {
    let payload = Bytes::from_slice(env, &command.encode());
    let result = SorobanVaultContract::execute(env.clone(), payload)?;
    VaultCommandResult::decode(&result.to_alloc_vec())
        .map_err(|_| templar_soroban_runtime::ContractError::InvalidInput)
}

struct VaultProxy<'a> {
    env: &'a Env,
}

impl<'a> VaultProxy<'a> {
    const fn new(env: &'a Env) -> Self {
        Self { env }
    }

    fn snapshot(&self, vault: &Address) -> (i128, i128, i128) {
        self.env.as_contract(vault, || {
            let core = SorobanVaultContract::proxy_view(
                self.env.clone(),
                Address::generate(self.env),
                0,
                0,
            )
            .unwrap()
            .0;
            (core.2 .0, core.2 .1, core.2 .2)
        })
    }

    fn initialize(&self, governance: &Address, asset: &Address, share: &Address) {
        SorobanVaultContract::initialize(
            self.env.clone(),
            governance.clone(),
            governance.clone(),
            asset.clone(),
            share.clone(),
            0,
            0,
        )
        .unwrap();
    }
}

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
        &1_000_000,
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
        .deposit(&deployer, &pool_addr, &500_000_000_000);
    pool_client.mock_all_auths().set_status(&3);
    pool_client.mock_all_auths().update_status();

    (pool_addr, pool_client, asset, asset_admin, deployer)
}

fn vault_snapshot(env: &Env, vault: &Address) -> (i128, i128, i128) {
    VaultProxy::new(env).snapshot(vault)
}

#[test]
#[allow(clippy::too_many_lines)]
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
    let proxy = VaultProxy::new(&env);

    env.as_contract(&vault, || {
        proxy.initialize(&governance, &asset, &share);
    });
    env.as_contract(&vault, || {
        let result = execute_command(
            &env,
            &VaultCommand::SetGovernanceConfig {
                caller: address_text(&governance),
                kind: GOVERNANCE_CONFIG_KIND_ALLOCATORS,
                primary: None,
                many: Some(vec![address_text(&allocator)]),
                value_a: None,
                value_b: None,
            },
        )
        .unwrap();
        assert!(matches!(result, VaultCommandResult::Unit));
    });
    env.as_contract(&vault, || {
        let result = execute_command(
            &env,
            &VaultCommand::SetGovernancePolicy {
                caller: address_text(&governance),
                kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                target_ids: Some(vec![0u32]),
                mode: None,
                accounts: None,
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        )
        .unwrap();
        assert!(matches!(result, VaultCommandResult::Unit));
    });
    env.as_contract(&vault, || {
        let result = execute_command(
            &env,
            &VaultCommand::SetGovernanceConfig {
                caller: address_text(&governance),
                kind: GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
                primary: None,
                many: Some(vec![address_text(&adapter)]),
                value_a: None,
                value_b: None,
            },
        )
        .unwrap();
        assert!(matches!(result, VaultCommandResult::Unit));
    });

    let deposit_amount = 10_000_000_000;
    let supply_amount = 6_000_000_000;
    let withdraw_amount = 2_500_000_000;

    asset_admin.mint(&user, &deposit_amount);

    let minted = env
        .as_contract(&vault, || {
            execute_command(
                &env,
                &VaultCommand::DepositWithMin {
                    owner: address_text(&user),
                    receiver: address_text(&user),
                    assets: deposit_amount,
                    min_shares_out: 0,
                },
            )
        })
        .unwrap();
    let VaultCommandResult::I128(minted) = minted else {
        panic!("expected i128 result")
    };
    assert_eq!(minted, deposit_amount);
    assert_eq!(
        vault_snapshot(&env, &vault),
        (deposit_amount, deposit_amount, 0)
    );

    let new_external = env
        .as_contract(&vault, || {
            execute_command(
                &env,
                &VaultCommand::Allocate {
                    caller: address_text(&allocator),
                    market: 0,
                    amount: supply_amount,
                    supply: true,
                },
            )
        })
        .unwrap();
    let VaultCommandResult::I128(new_external) = new_external else {
        panic!("expected i128 result")
    };
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
            execute_command(
                &env,
                &VaultCommand::RefreshMarkets {
                    caller: address_text(&allocator),
                    markets: vec![0u32],
                },
            )
        })
        .unwrap();
    let VaultCommandResult::I128(refreshed_external) = refreshed_external else {
        panic!("expected i128 result")
    };
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
            execute_command(
                &env,
                &VaultCommand::Allocate {
                    caller: address_text(&allocator),
                    market: 0,
                    amount: withdraw_amount,
                    supply: false,
                },
            )
        })
        .unwrap();
    let VaultCommandResult::I128(remaining_external) = remaining_external else {
        panic!("expected i128 result")
    };
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
