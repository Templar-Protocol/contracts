#![no_std]

extern crate alloc;

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Vec};

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
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    Unauthorized = 1,
    InvalidInput = 3,
    MissingConfig = 4,
    Reentrancy = 5,
    /// Arithmetic overflow when computing total assets.
    ArithmeticOverflow = 6,
    /// No supply position found for the given reserve index.
    MissingPosition = 7,
}

#[contract]
pub struct BlendAdapterContract;

#[contractimpl]
impl BlendAdapterContract {
    /// Runs atomically during contract deployment — no separate `initialize`
    /// transaction that could be front-run.
    pub fn __constructor(env: Env, admin: Address, vault: Address, pool: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage().instance().set(&DataKey::Pool, &pool);
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyLock, &false);
    }

    pub fn set_pool(env: Env, caller: Address, pool: Address) -> Result<(), AdapterError> {
        require_admin(&env, &caller)?;
        env.storage().instance().set(&DataKey::Pool, &pool);
        Ok(())
    }

    pub fn set_vault(env: Env, caller: Address, vault: Address) -> Result<(), AdapterError> {
        require_admin(&env, &caller)?;
        env.storage().instance().set(&DataKey::Vault, &vault);
        Ok(())
    }

    pub fn supply(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        // Adapter owns the Blend position. The vault should transfer assets to
        // the adapter before calling this method.
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }

        with_reentrancy_guard(&env, || {
            let pool = get_pool(&env)?;
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
            Ok(())
        })
    }

    pub fn withdraw(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        // Adapter owns the Blend position and transfers withdrawn assets back to the vault.
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }
        let vault = get_vault(&env)?;

        with_reentrancy_guard(&env, || {
            let pool = get_pool(&env)?;
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
            Ok(())
        })
    }

    pub fn rescue(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
        receiver: Address,
    ) -> Result<(), AdapterError> {
        // Move unexpected assets held by the adapter to a receiver.
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }

        with_reentrancy_guard(&env, || {
            let adapter = env.current_contract_address();
            let token = soroban_sdk::token::Client::new(&env, &asset);
            token.transfer(&adapter, &receiver, &amount);
            Ok(())
        })
    }

    pub fn total_assets(env: Env, asset: Address) -> Result<i128, AdapterError> {
        let pool = get_pool(&env)?;
        let client = PoolClient::new(&env, &pool);
        let reserve = client.get_reserve(&asset);
        let positions = client.get_positions(&env.current_contract_address());
        let index = reserve.config.index;
        let b_tokens = positions
            .supply
            .get(index)
            .ok_or(AdapterError::MissingPosition)?;
        b_tokens
            .checked_mul(reserve.data.b_rate)
            .and_then(|value| value.checked_div(SCALAR_12))
            .ok_or(AdapterError::ArithmeticOverflow)
    }

    pub fn admin(env: Env) -> Result<Address, AdapterError> {
        get_admin(&env)
    }

    pub fn vault(env: Env) -> Result<Address, AdapterError> {
        get_vault(&env)
    }

    pub fn pool(env: Env) -> Result<Address, AdapterError> {
        get_pool(&env)
    }
}

fn get_address(env: &Env, key: DataKey) -> Result<Address, AdapterError> {
    env.storage()
        .instance()
        .get(&key)
        .ok_or(AdapterError::MissingConfig)
}

fn get_admin(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Admin)
}

fn get_vault(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Vault)
}

fn get_pool(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Pool)
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let admin = get_admin(env)?;
    if caller != &admin {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn require_vault(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let vault = get_vault(env)?;
    if caller != &vault {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn with_reentrancy_guard<T>(
    env: &Env,
    f: impl FnOnce() -> Result<T, AdapterError>,
) -> Result<T, AdapterError> {
    let locked: bool = env
        .storage()
        .instance()
        .get(&DataKey::ReentrancyLock)
        .unwrap_or(false);
    if locked {
        return Err(AdapterError::Reentrancy);
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
    fn constructor_sets_config() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = Address::generate(&env);

        let contract_id = env.register(
            BlendAdapterContract,
            (&admin, &vault, &pool),
        );
        env.as_contract(&contract_id, || {
            assert_eq!(BlendAdapterContract::admin(env.clone()).unwrap(), admin);
            assert_eq!(BlendAdapterContract::vault(env.clone()).unwrap(), vault);
            assert_eq!(BlendAdapterContract::pool(env.clone()).unwrap(), pool);
        });
    }

    #[test]
    fn reentrancy_guard_blocks_nested() {
        let env = Env::default();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        env.as_contract(&contract_id, || {
            let result = with_reentrancy_guard(&env, || {
                with_reentrancy_guard(&env, || Ok(()))
            });
            assert_eq!(result, Err(AdapterError::Reentrancy));
        });
    }

    /// Helper: deploy a contract via constructor and return (contract_id, admin, vault, pool).
    fn setup_adapter(env: &Env) -> (Address, Address, Address, Address) {
        let admin = Address::generate(env);
        let vault = Address::generate(env);
        let pool = Address::generate(env);
        let contract_id = env.register(
            BlendAdapterContract,
            (&admin, &vault, &pool),
        );
        (contract_id, admin, vault, pool)
    }

    #[test]
    fn supply_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 0);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn supply_negative_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), -100);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn supply_unauthorized_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::supply(env.clone(), impostor.clone(), asset.clone(), 100);
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn withdraw_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 0);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn withdraw_negative_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), -50);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn withdraw_unauthorized_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::withdraw(env.clone(), impostor.clone(), asset.clone(), 100);
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn rescue_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let receiver = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                vault.clone(),
                asset.clone(),
                0,
                receiver.clone(),
            );
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn rescue_unauthorized_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        let receiver = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                impostor.clone(),
                asset.clone(),
                100,
                receiver.clone(),
            );
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn set_pool_updates_pool() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let new_pool = Address::generate(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_pool(env.clone(), admin.clone(), new_pool.clone()).unwrap();
            assert_eq!(BlendAdapterContract::pool(env.clone()).unwrap(), new_pool);
        });
    }

    #[test]
    fn set_vault_updates_vault() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let new_vault = Address::generate(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_vault(env.clone(), admin.clone(), new_vault.clone()).unwrap();
            assert_eq!(BlendAdapterContract::vault(env.clone()).unwrap(), new_vault);
        });
    }

    #[test]
    fn set_pool_unauthorized_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let impostor = Address::generate(&env);
        let new_pool = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::set_pool(env.clone(), impostor.clone(), new_pool.clone());
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn set_vault_unauthorized_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let impostor = Address::generate(&env);
        let new_vault = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::set_vault(env.clone(), impostor.clone(), new_vault.clone());
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    // Note: "query before initialize" test not applicable — __constructor
    // runs atomically during `env.register()`, so there is no uninitialized state.
}
