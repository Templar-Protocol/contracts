#![no_std]

extern crate alloc;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec};

use blend_contract_sdk::pool::{Client as PoolClient, Request};

const REQUEST_SUPPLY: u32 = 0;
const REQUEST_WITHDRAW: u32 = 1;
const SCALAR_12: i128 = 1_000_000_000_000;

#[contracttype]
#[derive(Clone, Debug)]
enum DataKey {
    Admin,
    Vault,
    Pool,
    ReentrancyLock,
    Initialized,
}

#[contract]
pub struct BlendAdapterContract;

#[contractimpl]
impl BlendAdapterContract {
    pub fn initialize(env: Env, admin: Address, vault: Address, pool: Address) {
        if env.storage().instance().has(&DataKey::Initialized) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage().instance().set(&DataKey::Pool, &pool);
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyLock, &false);
        env.storage().instance().set(&DataKey::Initialized, &true);
    }

    pub fn set_pool(env: Env, caller: Address, pool: Address) {
        caller.require_auth();
        let admin = get_admin(&env);
        if caller != admin {
            panic!("caller is not admin");
        }
        env.storage().instance().set(&DataKey::Pool, &pool);
    }

    pub fn set_vault(env: Env, caller: Address, vault: Address) {
        caller.require_auth();
        let admin = get_admin(&env);
        if caller != admin {
            panic!("caller is not admin");
        }
        env.storage().instance().set(&DataKey::Vault, &vault);
    }

    pub fn supply(env: Env, caller: Address, asset: Address, amount: i128) {
        // Adapter owns the Blend position. The vault should transfer assets to
        // the adapter before calling this method.
        caller.require_auth();
        let vault = get_vault(&env);
        if caller != vault {
            panic!("caller is not vault");
        }
        if amount <= 0 {
            panic!("amount must be positive");
        }

        with_reentrancy_guard(&env, || {
            let pool = get_pool(&env);
            let client = PoolClient::new(&env, &pool);
            let adapter = env.current_contract_address();
            let request = Request {
                request_type: REQUEST_SUPPLY,
                address: asset,
                amount,
            };
            let mut requests = Vec::new(&env);
            requests.push_back(request);
            client.submit(&adapter, &adapter, &adapter, &requests);
        });
    }

    pub fn withdraw(env: Env, caller: Address, asset: Address, amount: i128) {
        // Adapter owns the Blend position and transfers withdrawn assets back to the vault.
        caller.require_auth();
        let vault = get_vault(&env);
        if caller != vault {
            panic!("caller is not vault");
        }
        if amount <= 0 {
            panic!("amount must be positive");
        }

        with_reentrancy_guard(&env, || {
            let pool = get_pool(&env);
            let client = PoolClient::new(&env, &pool);
            let adapter = env.current_contract_address();
            let request = Request {
                request_type: REQUEST_WITHDRAW,
                address: asset.clone(),
                amount,
            };
            let mut requests = Vec::new(&env);
            requests.push_back(request);
            client.submit(&adapter, &adapter, &adapter, &requests);

            let token = soroban_sdk::token::Client::new(&env, &asset);
            token.transfer(&adapter, &vault, &amount);
        });
    }

    pub fn rescue(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
        receiver: Address,
    ) {
        // Move unexpected assets held by the adapter to a receiver.
        caller.require_auth();
        let vault = get_vault(&env);
        if caller != vault {
            panic!("caller is not vault");
        }
        if amount <= 0 {
            panic!("amount must be positive");
        }

        with_reentrancy_guard(&env, || {
            let adapter = env.current_contract_address();
            let token = soroban_sdk::token::Client::new(&env, &asset);
            token.transfer(&adapter, &receiver, &amount);
        });
    }

    pub fn total_assets(env: Env, asset: Address) -> i128 {
        let pool = get_pool(&env);
        let client = PoolClient::new(&env, &pool);
        let reserve = client.get_reserve(&asset);
        let positions = client.get_positions(&env.current_contract_address());
        let index = reserve.config.index;
        let b_tokens = positions.supply.get(index).unwrap_or(0);
        b_tokens
            .checked_mul(reserve.data.b_rate)
            .and_then(|value| value.checked_div(SCALAR_12))
            .unwrap_or(0)
    }

    pub fn admin(env: Env) -> Address {
        get_admin(&env)
    }

    pub fn vault(env: Env) -> Address {
        get_vault(&env)
    }

    pub fn pool(env: Env) -> Address {
        get_pool(&env)
    }
}

fn get_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("admin not set")
}

fn get_vault(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Vault)
        .expect("vault not set")
}

fn get_pool(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Pool)
        .expect("pool not set")
}

fn with_reentrancy_guard<T>(env: &Env, f: impl FnOnce() -> T) -> T {
    let locked: bool = env
        .storage()
        .instance()
        .get(&DataKey::ReentrancyLock)
        .unwrap_or(false);
    if locked {
        panic!("reentrancy");
    }
    env.storage()
        .instance()
        .set(&DataKey::ReentrancyLock, &true);
    let result = f();
    env.storage()
        .instance()
        .set(&DataKey::ReentrancyLock, &false);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn initialize_sets_config() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = Address::generate(&env);

        let contract_id = env.register_contract(None, BlendAdapterContract);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::initialize(
                env.clone(),
                admin.clone(),
                vault.clone(),
                pool.clone(),
            );
            assert_eq!(BlendAdapterContract::admin(env.clone()), admin);
            assert_eq!(BlendAdapterContract::vault(env.clone()), vault);
            assert_eq!(BlendAdapterContract::pool(env.clone()), pool);
        });
    }

    #[test]
    #[should_panic(expected = "reentrancy")]
    fn reentrancy_guard_blocks_nested() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = Address::generate(&env);

        let contract_id = env.register_contract(None, BlendAdapterContract);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::initialize(
                env.clone(),
                admin.clone(),
                vault.clone(),
                pool.clone(),
            );
            with_reentrancy_guard(&env, || {
                with_reentrancy_guard(&env, || {});
            });
        });
    }
}
