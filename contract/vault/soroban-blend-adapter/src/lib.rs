#![no_std]

extern crate alloc;

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, IntoVal,
    Symbol, Vec,
};

use blend_contract_sdk::pool::{Client as PoolClient, Request};

const REQUEST_SUPPLY: u32 = 0;
const REQUEST_WITHDRAW: u32 = 1;
const SCALAR_12: i128 = 1_000_000_000_000;
/// Re-extend instance TTL when remaining TTL drops below ~30 days.
const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
/// Extend instance TTL to the Soroban maximum (~6 months).
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
/// Maximum allowed staleness for reserve data in seconds.
const RESERVE_STALE_WINDOW_SECS: u64 = 300;

#[contracttype]
#[derive(Clone, Debug)]
enum DataKey {
    Admin,
    Vault,
    Pool,
    ReentrancyLock,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterEvent {
    Supply { asset: Address, amount: i128 },
    Withdraw { asset: Address, amount: i128 },
    Rescue {
        asset: Address,
        amount: i128,
        receiver: Address,
    },
    PoolUpdated { old_pool: Address, new_pool: Address },
    VaultUpdated { old_vault: Address, new_vault: Address },
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
    /// Arithmetic underflow when computing actual withdrawal.
    ArithmeticUnderflow = 8,
    /// Withdrawal returned zero assets.
    ZeroWithdrawal = 9,
    /// Reserve data is stale.
    StaleReserve = 10,
}

#[contract]
pub struct BlendAdapterContract;

#[contractimpl]
impl BlendAdapterContract {
    /// Runs atomically during contract deployment — no separate `initialize`
    /// transaction that could be front-run.
    pub fn __constructor(env: Env, admin: Address, vault: Address, pool: Address) {
        extend_instance_ttl(&env);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage().instance().set(&DataKey::Pool, &pool);
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyLock, &false);
    }

    /// Update the Blend pool contract address (admin-only).
    ///
    /// # Preconditions
    /// - `caller` must be the admin.
    /// - `pool` must be a contract address.
    ///
    /// Emits `PoolUpdated` on success.
    pub fn set_pool(env: Env, caller: Address, pool: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_contract_address(&pool, AdapterError::InvalidInput)?;
        let old_pool = get_pool(&env)?;
        env.storage().instance().set(&DataKey::Pool, &pool);
        env.events().publish(
            (symbol_short!("pool_updated"),),
            AdapterEvent::PoolUpdated {
                old_pool,
                new_pool: pool,
            },
        );
        Ok(())
    }

    /// Update the vault contract address (admin-only).
    ///
    /// # Preconditions
    /// - `caller` must be the admin.
    /// - `vault` must be a contract address.
    ///
    /// Emits `VaultUpdated` on success.
    pub fn set_vault(env: Env, caller: Address, vault: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_contract_address(&vault, AdapterError::InvalidInput)?;
        let old_vault = get_vault(&env)?;
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.events().publish(
            (symbol_short!("vault_updated"),),
            AdapterEvent::VaultUpdated {
                old_vault,
                new_vault: vault,
            },
        );
        Ok(())
    }

    /// Supply assets from the adapter into the Blend pool (vault-only).
    ///
    /// # Preconditions
    /// - `caller` must be the configured vault.
    /// - `amount` must be positive.
    /// - The vault must have transferred `amount` of `asset` to the adapter
    ///   prior to calling this method.
    ///
    /// Emits `Supply` on success.
    pub fn supply(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
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
                address: asset.clone(),
                amount,
            };
            let mut requests = Vec::new(&env);
            requests.push_back(request);

            // Authorize the token transfer the pool will make from the adapter.
            env.authorize_as_current_contract(Vec::from_array(
                &env,
                [InvokerContractAuthEntry::Contract(SubContractInvocation {
                    context: ContractContext {
                        contract: asset.clone(),
                        fn_name: Symbol::new(&env, "transfer"),
                        args: (adapter.clone(), pool.clone(), amount).into_val(&env),
                    },
                    sub_invocations: Vec::new(&env),
                })],
            ));

            client.submit(&adapter, &adapter, &adapter, &requests);
            env.events().publish(
                (symbol_short!("supply"),),
                AdapterEvent::Supply { asset, amount },
            );
            Ok(())
        })
    }

    /// Withdraw assets from the Blend pool and transfer them to the vault.
    ///
    /// # Preconditions
    /// - `caller` must be the configured vault.
    /// - `amount` must be positive.
    ///
    /// If the pool returns fewer assets than requested, the adapter forwards
    /// the actual amount received. Emits `Withdraw` with the actual amount.
    pub fn withdraw(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
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
            let token = soroban_sdk::token::Client::new(&env, &asset);
            let balance_before = token.balance(&adapter);
            client.submit(&adapter, &adapter, &adapter, &requests);

            let balance_after = token.balance(&adapter);
            let actual_withdrawn = balance_after
                .checked_sub(balance_before)
                .ok_or(AdapterError::ArithmeticUnderflow)?;
            if actual_withdrawn <= 0 {
                return Err(AdapterError::ZeroWithdrawal);
            }
            token.transfer(&adapter, &vault, &actual_withdrawn);
            env.events().publish(
                (symbol_short!("withdraw"),),
                AdapterEvent::Withdraw {
                    asset,
                    amount: actual_withdrawn,
                },
            );
            Ok(())
        })
    }

    /// Rescue assets held by the adapter and transfer them to `receiver`.
    ///
    /// # Preconditions
    /// - `caller` must be the configured vault.
    /// - `amount` must be positive.
    /// - `receiver` must be a contract address and not the adapter itself.
    ///
    /// Emits `Rescue` on success.
    pub fn rescue(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
        receiver: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        // Move unexpected assets held by the adapter to a receiver.
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }
        require_contract_address(&receiver, AdapterError::InvalidInput)?;
        if receiver == env.current_contract_address() {
            return Err(AdapterError::InvalidInput);
        }

        with_reentrancy_guard(&env, || {
            let adapter = env.current_contract_address();
            let token = soroban_sdk::token::Client::new(&env, &asset);
            token.transfer(&adapter, &receiver, &amount);
            env.events().publish(
                (symbol_short!("rescue"),),
                AdapterEvent::Rescue {
                    asset,
                    amount,
                    receiver,
                },
            );
            Ok(())
        })
    }

    /// Query total assets for `asset` from the Blend pool.
    ///
    /// Returns an error if reserve data is stale, positions are missing, or
    /// arithmetic overflows.
    pub fn total_assets(env: Env, asset: Address) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        let pool = get_pool(&env)?;
        let client = PoolClient::new(&env, &pool);
        let reserve = client.get_reserve(&asset);
        let now = env.ledger().timestamp();
        let last_update = reserve.data.last_time as u64;
        if now.saturating_sub(last_update) > RESERVE_STALE_WINDOW_SECS {
            return Err(AdapterError::StaleReserve);
        }
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
        extend_instance_ttl(&env);
        get_admin(&env)
    }

    pub fn vault(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_vault(&env)
    }

    pub fn pool(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_pool(&env)
    }

    /// Extend instance storage TTL (admin-only).
    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        Ok(())
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

fn is_contract_address(addr: &Address) -> bool {
    let bytes = addr.to_string().to_bytes();
    matches!(bytes.get(0), Some(b'C'))
}

fn require_contract_address(
    addr: &Address,
    err: AdapterError,
) -> Result<(), AdapterError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(err)
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

fn with_reentrancy_guard<T>(
    env: &Env,
    f: impl FnOnce() -> Result<T, AdapterError>,
) -> Result<T, AdapterError> {
    let _guard = ReentrancyGuard::new(env)?;
    f()
}

struct ReentrancyGuard<'a> {
    env: &'a Env,
}

impl<'a> ReentrancyGuard<'a> {
    fn new(env: &'a Env) -> Result<Self, AdapterError> {
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
        Ok(Self { env })
    }
}

impl Drop for ReentrancyGuard<'_> {
    fn drop(&mut self) {
        self.env
            .storage()
            .instance()
            .set(&DataKey::ReentrancyLock, &false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events as _},
        token::StellarAssetClient,
        vec, IntoVal, TryFromVal, Val, symbol_short,
    };

    #[contract]
    struct MockPoolContract;

    #[contractimpl]
    impl MockPoolContract {
        pub fn submit(
            env: Env,
            from: Address,
            _spender: Address,
            _receiver: Address,
            requests: Vec<Request>,
        ) {
            let pool = env.current_contract_address();
            for request in requests.iter() {
                let token = soroban_sdk::token::Client::new(&env, &request.address);
                if request.request_type == REQUEST_SUPPLY {
                    token.transfer(&from, &pool, &request.amount);
                } else if request.request_type == REQUEST_WITHDRAW {
                    let available = token.balance(&pool);
                    let to_transfer = request.amount.min(available);
                    if to_transfer > 0 {
                        token.transfer(&pool, &from, &to_transfer);
                    }
                }
            }
        }
    }

    fn adapter_events(env: &Env, adapter: &Address) -> std::vec::Vec<(Address, Vec<Val>, Val)> {
        env.events()
            .all()
            .into_iter()
            .filter(|event| &event.0 == adapter)
            .collect()
    }

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
    fn rescue_rejects_adapter_receiver() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                vault.clone(),
                asset.clone(),
                100,
                contract_id.clone(),
            );
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn rescue_rejects_non_contract_receiver() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let account = Address::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        );
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                vault.clone(),
                asset.clone(),
                100,
                account,
            );
            assert_eq!(result, Err(AdapterError::InvalidInput));
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
    fn set_pool_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, old_pool) = setup_adapter(&env);
        let new_pool = Address::generate(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_pool(env.clone(), admin, new_pool.clone()).unwrap();
        });
        let events = adapter_events(&env, &contract_id);
        assert_eq!(events.len(), 1);
        let event = events.get(0).unwrap();
        assert_eq!(
            event.1,
            vec![&env, symbol_short!("pool_updated").into_val(&env)]
        );
        let payload: AdapterEvent = AdapterEvent::try_from_val(&env, &event.2).unwrap();
        assert_eq!(
            payload,
            AdapterEvent::PoolUpdated {
                old_pool,
                new_pool
            }
        );
    }

    #[test]
    fn set_vault_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, old_vault, _pool) = setup_adapter(&env);
        let new_vault = Address::generate(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_vault(env.clone(), admin, new_vault.clone()).unwrap();
        });
        let events = adapter_events(&env, &contract_id);
        assert_eq!(events.len(), 1);
        let event = events.get(0).unwrap();
        assert_eq!(
            event.1,
            vec![&env, symbol_short!("vault_updated").into_val(&env)]
        );
        let payload: AdapterEvent = AdapterEvent::try_from_val(&env, &event.2).unwrap();
        assert_eq!(
            payload,
            AdapterEvent::VaultUpdated {
                old_vault,
                new_vault
            }
        );
    }

    #[test]
    fn supply_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = env.register(MockPoolContract, ());
        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));

        let token = env.register_stellar_asset_contract_v2(admin.clone());
        let asset = token.address();
        let token_client = StellarAssetClient::new(&env, &asset);
        token_client
            .mock_all_auths()
            .mint(&contract_id, &1_000);

        env.as_contract(&contract_id, || {
            BlendAdapterContract::supply(env.clone(), vault, asset.clone(), 250).unwrap();
        });

        let events = adapter_events(&env, &contract_id);
        assert_eq!(events.len(), 1);
        let event = events.get(0).unwrap();
        assert_eq!(
            event.1,
            vec![&env, symbol_short!("supply").into_val(&env)]
        );
        let payload: AdapterEvent = AdapterEvent::try_from_val(&env, &event.2).unwrap();
        assert_eq!(
            payload,
            AdapterEvent::Supply {
                asset,
                amount: 250
            }
        );
    }

    #[test]
    fn withdraw_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = env.register(MockPoolContract, ());
        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));

        let token = env.register_stellar_asset_contract_v2(admin.clone());
        let asset = token.address();
        let token_client = StellarAssetClient::new(&env, &asset);
        token_client
            .mock_all_auths()
            .mint(&pool, &5_000);

        env.as_contract(&contract_id, || {
            BlendAdapterContract::withdraw(env.clone(), vault, asset.clone(), 400).unwrap();
        });

        let events = adapter_events(&env, &contract_id);
        assert_eq!(events.len(), 1);
        let event = events.get(0).unwrap();
        assert_eq!(
            event.1,
            vec![&env, symbol_short!("withdraw").into_val(&env)]
        );
        let payload: AdapterEvent = AdapterEvent::try_from_val(&env, &event.2).unwrap();
        assert_eq!(
            payload,
            AdapterEvent::Withdraw {
                asset,
                amount: 400
            }
        );
    }

    #[test]
    fn withdraw_handles_partial_liquidity() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = env.register(MockPoolContract, ());
        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));

        let token = env.register_stellar_asset_contract_v2(admin.clone());
        let asset = token.address();
        let token_client = StellarAssetClient::new(&env, &asset);
        token_client
            .mock_all_auths()
            .mint(&pool, &300);

        let vault_before = token_client.balance(&vault);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 1_000)
                .unwrap();
        });
        let vault_after = token_client.balance(&vault);
        assert_eq!(vault_after - vault_before, 300);

        let events = adapter_events(&env, &contract_id);
        assert_eq!(events.len(), 1);
        let event = events.get(0).unwrap();
        let payload: AdapterEvent = AdapterEvent::try_from_val(&env, &event.2).unwrap();
        assert_eq!(
            payload,
            AdapterEvent::Withdraw {
                asset,
                amount: 300
            }
        );
    }

    #[test]
    fn rescue_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let vault = Address::generate(&env);
        let pool = Address::generate(&env);
        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));

        let token = env.register_stellar_asset_contract_v2(admin.clone());
        let asset = token.address();
        let token_client = StellarAssetClient::new(&env, &asset);
        token_client
            .mock_all_auths()
            .mint(&contract_id, &2_000);
        let receiver = Address::generate(&env);

        env.as_contract(&contract_id, || {
            BlendAdapterContract::rescue(env.clone(), vault, asset.clone(), 300, receiver.clone())
                .unwrap();
        });

        let events = adapter_events(&env, &contract_id);
        assert_eq!(events.len(), 1);
        let event = events.get(0).unwrap();
        assert_eq!(
            event.1,
            vec![&env, symbol_short!("rescue").into_val(&env)]
        );
        let payload: AdapterEvent = AdapterEvent::try_from_val(&env, &event.2).unwrap();
        assert_eq!(
            payload,
            AdapterEvent::Rescue {
                asset,
                amount: 300,
                receiver
            }
        );
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
    fn set_pool_rejects_non_contract_address() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let account = Address::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        );
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::set_pool(env.clone(), admin, account);
            assert_eq!(result, Err(AdapterError::InvalidInput));
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

    #[test]
    fn set_vault_rejects_non_contract_address() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let account = Address::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        );
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::set_vault(env.clone(), admin, account);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    // Note: "query before initialize" test not applicable — __constructor
    // runs atomically during `env.register()`, so there is no uninitialized state.
}
