#![no_std]

#[cfg(any(test, feature = "testutils"))]
extern crate std;

use soroban_sdk::{
    address_payload::AddressPayload, contract, contracterror, contractimpl, contracttype,
    panic_with_error, symbol_short, Address, BytesN, Env,
};
use stellar_contract_utils::upgradeable::{self, Upgradeable};

/// Re-extend instance TTL when remaining TTL drops below ~30 days.
const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
/// Extend instance TTL to the Soroban maximum (~6 months).
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;

#[contracttype]
#[derive(Clone, Debug)]
enum DataKey {
    Admin,
    PendingAdmin,
    Vault,
    Custodian,
    Paused,
    ReportedAssets(Address),
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    ArithmeticOverflow = 4,
    ArithmeticUnderflow = 5,
    InsufficientReturnedLiquidity = 6,
    Paused = 7,
}

#[contract]
pub struct CustodialAdapterContract;

#[contractimpl]
impl CustodialAdapterContract {
    /// Configure the adapter.
    ///
    /// The `custodian` may be either an account or contract address. It is the
    /// operational address that receives allocated funds and is expected to
    /// return assets to this adapter before vault withdrawals are progressed.
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        custodian: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault, AdapterError::InvalidInput)?;
        let adapter = env.current_contract_address();
        if custodian == vault || custodian == adapter {
            return Err(AdapterError::InvalidInput);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage()
            .instance()
            .set(&DataKey::Custodian, &custodian);
        Ok(())
    }

    /// Pause or unpause vault allocation and withdrawal operations.
    #[allow(deprecated)]
    pub fn set_paused(env: Env, caller: Address, paused: bool) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin_or_vault(&env, &caller)?;
        env.storage().instance().set(&DataKey::Paused, &paused);
        env.events()
            .publish((symbol_short!("paused"), caller), paused);
        Ok(())
    }

    pub fn paused(env: Env) -> bool {
        extend_instance_ttl(&env);
        is_paused(&env)
    }

    /// Forward allocated funds to the configured custodian and increase the
    /// adapter's reported market assets by `amount`.
    #[allow(deprecated)]
    pub fn supply(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        require_positive(amount)?;

        let adapter = env.current_contract_address();
        let custodian = get_custodian(&env)?;
        let token = soroban_sdk::token::Client::new(&env, &asset);
        token.transfer(&adapter, &custodian, &amount);

        let reported = load_reported_assets(&env, &asset);
        let next = reported
            .checked_add(amount)
            .ok_or(AdapterError::ArithmeticOverflow)?;
        store_reported_assets(&env, &asset, next);

        env.events()
            .publish((symbol_short!("supply"), asset, custodian), amount);
        Ok(())
    }

    /// Withdraw returned idle liquidity from the adapter to the vault.
    #[allow(deprecated)]
    pub fn withdraw(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        Self::progress_withdrawal(env, caller, asset, amount).map(|_| ())
    }

    /// Progress a vault withdrawal using only assets already returned to this
    /// adapter. This method does not initiate any offchain market exit.
    #[allow(deprecated)]
    pub fn progress_withdrawal(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        require_positive(amount)?;

        let adapter = env.current_contract_address();
        let token = soroban_sdk::token::Client::new(&env, &asset);
        let idle_balance = token.balance(&adapter);
        let reported = load_reported_assets(&env, &asset);
        let (actual, next_reported) = withdrawal_result(reported, idle_balance, amount)?;
        let vault = get_vault(&env)?;
        token.transfer(&adapter, &vault, &actual);
        store_reported_assets(&env, &asset, next_reported);

        env.events()
            .publish((symbol_short!("withdraw"), asset), actual);
        Ok(actual)
    }

    /// Return the adapter's reported assets for `asset`.
    ///
    /// Returned idle balance is intentionally not auto-added here. The custodian
    /// address may return principal or yield, but this adapter cannot prove the
    /// offchain source of funds. Operators should use `set_reported_assets` for
    /// explicit NAV updates when needed.
    pub fn total_assets(env: Env, asset: Address) -> i128 {
        extend_instance_ttl(&env);
        load_reported_assets(&env, &asset)
    }

    /// Explicitly set reported market assets for `asset`.
    ///
    /// The configured custodian is allowed to report NAV because custody of the
    /// offchain position is already part of this adapter's trust boundary. This
    /// keeps reporting usable when the admin is a governance contract.
    #[allow(deprecated)]
    pub fn set_reported_assets(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_reporter(&env, &caller)?;
        if amount < 0 {
            return Err(AdapterError::InvalidInput);
        }
        store_reported_assets(&env, &asset, amount);
        env.events()
            .publish((symbol_short!("report"), caller, asset), amount);
        Ok(())
    }

    pub fn admin(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_admin(&env)
    }

    pub fn pending_admin(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_pending_admin(&env)
    }

    pub fn vault(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_vault(&env)
    }

    pub fn custodian(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_custodian(&env)
    }

    /// Propose a new admin. The pending admin must accept in a separate call.
    #[allow(deprecated)]
    pub fn set_admin(env: Env, caller: Address, admin: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage().instance().set(&DataKey::PendingAdmin, &admin);
        env.events()
            .publish((symbol_short!("admin_set"), caller), admin);
        Ok(())
    }

    /// Accept admin role previously proposed with `set_admin`.
    #[allow(deprecated)]
    pub fn accept_admin(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        caller.require_auth();
        let pending_admin = get_pending_admin(&env)?;
        if caller != pending_admin {
            return Err(AdapterError::Unauthorized);
        }
        let old_admin = get_admin(&env)?;
        env.storage().instance().set(&DataKey::Admin, &caller);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        env.events()
            .publish((symbol_short!("admin_acc"), old_admin), caller);
        Ok(())
    }

    /// Extend instance storage TTL (admin-only).
    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        Ok(())
    }
}

#[contractimpl]
impl Upgradeable for CustodialAdapterContract {
    #[allow(deprecated)]
    fn upgrade(e: &Env, new_wasm_hash: BytesN<32>, operator: Address) {
        extend_instance_ttl(e);
        require_admin(e, &operator).unwrap_or_else(|err| panic_with_error!(e, err));
        upgradeable::upgrade(e, &new_wasm_hash);
        e.events()
            .publish((symbol_short!("upgrade"), operator), new_wasm_hash);
    }
}

#[cfg(any(test, feature = "testutils"))]
pub fn simulate_progress_withdrawal(
    reported: i128,
    idle_balance: i128,
    requested: i128,
) -> Result<(i128, i128), AdapterError> {
    withdrawal_result(reported, idle_balance, requested)
}

fn withdrawal_result(
    reported: i128,
    idle_balance: i128,
    requested: i128,
) -> Result<(i128, i128), AdapterError> {
    require_positive(requested)?;
    if reported < 0 || idle_balance < 0 {
        return Err(AdapterError::InvalidInput);
    }
    let actual = min_i128(min_i128(reported, idle_balance), requested);
    if actual <= 0 {
        return Err(AdapterError::InsufficientReturnedLiquidity);
    }
    let next_reported = reported
        .checked_sub(actual)
        .ok_or(AdapterError::ArithmeticUnderflow)?;
    Ok((actual, next_reported))
}

fn min_i128(a: i128, b: i128) -> i128 {
    if a < b {
        a
    } else {
        b
    }
}

fn require_positive(amount: i128) -> Result<(), AdapterError> {
    if amount <= 0 {
        Err(AdapterError::InvalidInput)
    } else {
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

fn get_pending_admin(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::PendingAdmin)
}

fn get_vault(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Vault)
}

fn get_custodian(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Custodian)
}

fn load_reported_assets(env: &Env, asset: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::ReportedAssets(asset.clone()))
        .unwrap_or(0)
}

fn store_reported_assets(env: &Env, asset: &Address, amount: i128) {
    env.storage()
        .instance()
        .set(&DataKey::ReportedAssets(asset.clone()), &amount);
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

fn require_admin_or_vault(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let admin = get_admin(env)?;
    let vault = get_vault(env)?;
    if caller != &admin && caller != &vault {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn require_reporter(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let admin = get_admin(env)?;
    let vault = get_vault(env)?;
    let custodian = get_custodian(env)?;
    if caller != &admin && caller != &vault && caller != &custodian {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_not_paused(env: &Env) -> Result<(), AdapterError> {
    if is_paused(env) {
        return Err(AdapterError::Paused);
    }
    Ok(())
}

fn is_contract_address(addr: &Address) -> bool {
    matches!(
        AddressPayload::from_address(addr),
        Some(AddressPayload::ContractIdHash(_))
    )
}

fn require_contract_address(addr: &Address, err: AdapterError) -> Result<(), AdapterError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::testutils::{Address as _, Events as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::{contract, contractimpl, IntoVal, Symbol};

    const ADAPTER_SOURCE: &str = include_str!("lib.rs");

    #[contract]
    struct DummyContract;

    #[contractimpl]
    impl DummyContract {}

    fn register_dummy_contract(env: &Env) -> Address {
        env.register(DummyContract, ())
    }

    fn account_address(env: &Env) -> Address {
        Address::from_str(
            env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
    }

    fn setup_adapter(env: &Env) -> (Address, Address, Address, Address) {
        let admin = account_address(env);
        let vault = register_dummy_contract(env);
        let custodian = Address::generate(env);
        let contract_id = env.register(CustodialAdapterContract, (&admin, &vault, &custodian));
        (contract_id, admin, vault, custodian)
    }

    fn token_balance(env: &Env, token: &Address, account: &Address) -> i128 {
        soroban_sdk::token::Client::new(env, token).balance(account)
    }

    fn adapter_event_count(env: &Env, contract_id: &Address) -> usize {
        env.events()
            .all()
            .filter_by_contract(contract_id)
            .events()
            .len()
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
    fn constructor_sets_config_and_allows_account_custodian() {
        let env = Env::default();
        let (contract_id, admin, vault, custodian) = setup_adapter(&env);
        env.as_contract(&contract_id, || {
            assert_eq!(CustodialAdapterContract::admin(env.clone()).unwrap(), admin);
            assert_eq!(CustodialAdapterContract::vault(env.clone()).unwrap(), vault);
            assert_eq!(
                CustodialAdapterContract::custodian(env.clone()).unwrap(),
                custodian
            );
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_non_contract_vault() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = account_address(&env);
        let custodian = Address::generate(&env);
        env.register(CustodialAdapterContract, (&admin, &vault, &custodian));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_custodian_equal_to_vault() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = register_dummy_contract(&env);
        env.register(CustodialAdapterContract, (&admin, &vault, &vault));
    }

    #[test]
    fn supply_forwards_funds_and_reports_assets() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian) = setup_adapter(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1_000);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::supply(env.clone(), vault, asset.clone(), 750).unwrap();
            assert_eq!(
                CustodialAdapterContract::total_assets(env.clone(), asset.clone()),
                750
            );
        });

        assert_eq!(token_balance(&env, &asset, &custodian), 750);
        assert_eq!(token_balance(&env, &asset, &contract_id), 250);
    }

    #[test]
    fn supply_requires_configured_vault() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _custodian) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), impostor, asset, 1),
                Err(AdapterError::Unauthorized)
            );
        });
    }

    #[test]
    fn supply_with_exact_vault_auth_transfers_adapter_funds() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian) = setup_adapter(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1_000);

        let args: soroban_sdk::Vec<soroban_sdk::Val> = (&vault, &asset, &750i128).into_val(&env);
        env.mock_auths(&[MockAuth {
            address: &vault,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "supply",
                args: args.clone(),
                sub_invokes: &[],
            },
        }]);
        let result = env.try_invoke_contract::<(), AdapterError>(
            &contract_id,
            &Symbol::new(&env, "supply"),
            args,
        );

        assert_eq!(result, Ok(Ok(())));
        assert_eq!(token_balance(&env, &asset, &custodian), 750);
    }

    #[test]
    fn withdrawal_releases_only_returned_liquidity() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian) = setup_adapter(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1_000);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 1_000)
                .unwrap();
        });

        asset_admin.mint(&custodian, &400);
        let token = soroban_sdk::token::Client::new(&env, &asset);
        token
            .mock_all_auths()
            .transfer(&custodian, &contract_id, &400);

        env.as_contract(&contract_id, || {
            let actual = CustodialAdapterContract::progress_withdrawal(
                env.clone(),
                vault.clone(),
                asset.clone(),
                700,
            )
            .unwrap();
            assert_eq!(actual, 400);
            assert_eq!(
                CustodialAdapterContract::total_assets(env.clone(), asset.clone()),
                600
            );
        });
        assert_eq!(token_balance(&env, &asset, &vault), 400);
        assert_eq!(token_balance(&env, &asset, &contract_id), 0);
    }

    #[test]
    fn withdrawal_requires_configured_vault() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _custodian) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(env.clone(), impostor, asset, 1),
                Err(AdapterError::Unauthorized)
            );
        });
    }

    #[test]
    fn withdrawal_with_exact_vault_auth_transfers_adapter_funds() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian) = setup_adapter(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &500);
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                vault.clone(),
                asset.clone(),
                500,
            )
            .unwrap();
        });

        let args: soroban_sdk::Vec<soroban_sdk::Val> = (&vault, &asset, &300i128).into_val(&env);
        env.mock_auths(&[MockAuth {
            address: &vault,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "progress_withdrawal",
                args: args.clone(),
                sub_invokes: &[],
            },
        }]);
        let result = env.try_invoke_contract::<i128, AdapterError>(
            &contract_id,
            &Symbol::new(&env, "progress_withdrawal"),
            args,
        );

        assert_eq!(result, Ok(Ok(300)));
        assert_eq!(token_balance(&env, &asset, &vault), 300);
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::total_assets(env.clone(), asset.clone()),
                200
            );
        });
    }

    #[test]
    fn withdrawal_fails_when_no_liquidity_has_returned() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian) = setup_adapter(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                vault.clone(),
                asset.clone(),
                1_000,
            )
            .unwrap();
        });
        env.as_contract(&contract_id, || {
            let result =
                CustodialAdapterContract::progress_withdrawal(env.clone(), vault, asset, 100);
            assert_eq!(result, Err(AdapterError::InsufficientReturnedLiquidity));
        });
    }

    #[test]
    fn withdrawal_does_not_exceed_reported_assets() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian) = setup_adapter(&env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1_000);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                vault.clone(),
                asset.clone(),
                250,
            )
            .unwrap();
        });
        env.as_contract(&contract_id, || {
            let actual = CustodialAdapterContract::progress_withdrawal(
                env.clone(),
                vault.clone(),
                asset.clone(),
                500,
            )
            .unwrap();
            assert_eq!(actual, 250);
            assert_eq!(
                CustodialAdapterContract::total_assets(env.clone(), asset.clone()),
                0
            );
        });
        assert_eq!(token_balance(&env, &asset, &vault), 250);
        assert_eq!(token_balance(&env, &asset, &contract_id), 750);
    }

    #[test]
    fn set_reported_assets_requires_admin_vault_or_custodian() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, custodian) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    impostor,
                    asset.clone(),
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
            CustodialAdapterContract::set_reported_assets(env.clone(), admin, asset.clone(), 9)
                .unwrap();
            CustodialAdapterContract::set_reported_assets(env.clone(), vault, asset.clone(), 7)
                .unwrap();
            CustodialAdapterContract::set_reported_assets(env.clone(), custodian, asset.clone(), 5)
                .unwrap();
            assert_eq!(
                CustodialAdapterContract::total_assets(env.clone(), asset.clone()),
                5
            );
        });
    }

    #[test]
    fn negative_or_zero_amounts_are_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 0),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    -1
                ),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(env.clone(), vault, asset, -1),
                Err(AdapterError::InvalidInput)
            );
        });
    }

    #[test]
    fn pause_blocks_supply_withdraw_and_report_updates() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, _custodian) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_paused(env.clone(), admin, true).unwrap();
            assert!(CustodialAdapterContract::paused(env.clone()));
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 1),
                Err(AdapterError::Paused)
            );
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    1
                ),
                Err(AdapterError::Paused)
            );
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(env.clone(), vault, asset, 1),
                Err(AdapterError::Paused)
            );
        });
    }

    #[test]
    fn set_admin_requires_pending_admin_acceptance() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _custodian) = setup_adapter(&env);
        let new_admin = Address::generate(&env);
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_admin(env.clone(), admin.clone(), new_admin.clone())
                .unwrap();
            assert_eq!(CustodialAdapterContract::admin(env.clone()).unwrap(), admin);
            assert_eq!(
                CustodialAdapterContract::pending_admin(env.clone()).unwrap(),
                new_admin
            );
            CustodialAdapterContract::accept_admin(env.clone(), new_admin.clone()).unwrap();
            assert_eq!(
                CustodialAdapterContract::admin(env.clone()).unwrap(),
                new_admin
            );
            assert_eq!(
                CustodialAdapterContract::pending_admin(env.clone()),
                Err(AdapterError::MissingConfig)
            );
        });
    }

    #[test]
    fn admin_events_and_upgrade_emit() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _custodian) = setup_adapter(&env);
        let new_admin = Address::generate(&env);
        let new_hash = empty_wasm_hash(&env);
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_admin(env.clone(), admin.clone(), new_admin.clone())
                .unwrap();
            CustodialAdapterContract::accept_admin(env.clone(), new_admin.clone()).unwrap();
        });
        assert_eq!(adapter_event_count(&env, &contract_id), 2);
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::upgrade(&env, new_hash, new_admin);
        });
        assert_eq!(adapter_event_count(&env, &contract_id), 1);
    }

    #[test]
    fn no_public_custody_or_vault_retarget_entrypoints() {
        assert!(!ADAPTER_SOURCE.contains(concat!("pub fn ", "set_custodian")));
        assert!(!ADAPTER_SOURCE.contains(concat!("pub fn ", "set_vault")));
        assert!(!ADAPTER_SOURCE.contains(concat!("pub fn ", "rescue")));
    }

    proptest! {
        #[test]
        fn withdrawal_model_never_overdraws(
            reported in 0i128..=1_000_000_000_000i128,
            idle in 0i128..=1_000_000_000_000i128,
            requested in 1i128..=1_000_000_000_000i128,
        ) {
            let result = simulate_progress_withdrawal(reported, idle, requested);
            if reported == 0 || idle == 0 {
                prop_assert_eq!(result, Err(AdapterError::InsufficientReturnedLiquidity));
            } else {
                let (actual, next_reported) = result.expect("positive reported and idle should withdraw");
                prop_assert!(actual > 0);
                prop_assert!(actual <= reported);
                prop_assert!(actual <= idle);
                prop_assert!(actual <= requested);
                prop_assert_eq!(next_reported, reported - actual);
            }
        }

        #[test]
        fn supply_then_withdraw_model_preserves_non_negative_report(
            initial_reported in 0i128..=1_000_000_000i128,
            supplied in 1i128..=1_000_000_000i128,
            returned in 0i128..=1_000_000_000i128,
            requested in 1i128..=1_000_000_000i128,
        ) {
            let reported = initial_reported + supplied;
            let result = simulate_progress_withdrawal(reported, returned, requested);
            if returned == 0 {
                prop_assert_eq!(result, Err(AdapterError::InsufficientReturnedLiquidity));
            } else {
                let (actual, next_reported) = result.expect("returned liquidity should withdraw");
                prop_assert_eq!(next_reported + actual, reported);
                prop_assert!(next_reported >= 0);
            }
        }
    }
}
