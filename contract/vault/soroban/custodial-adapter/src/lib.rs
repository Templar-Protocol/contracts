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
    Asset,
    Paused,
    ReportedAssets(Address),
    ReportNonce(Address),
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
        asset: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault, AdapterError::InvalidInput)?;
        require_contract_address(&asset, AdapterError::InvalidInput)?;
        let adapter = env.current_contract_address();
        if admin == adapter
            || vault == adapter
            || asset == adapter
            || asset == admin
            || asset == vault
            || asset == custodian
            || custodian == vault
            || custodian == adapter
        {
            return Err(AdapterError::InvalidInput);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage()
            .instance()
            .set(&DataKey::Custodian, &custodian);
        env.storage().instance().set(&DataKey::Asset, &asset);
        Ok(())
    }

    /// Pause or unpause new vault allocation and reported-NAV update operations.
    ///
    /// While paused, vault and custodian report updates are blocked. The admin
    /// may still call `set_reported_assets` to correct NAV during recovery.
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
    /// adapter's reported route NAV by `amount`.
    ///
    /// Reported route NAV is the amount attributed to this adapter that has not
    /// yet been released back to the vault. It includes both offchain position
    /// value and any returned idle liquidity still held by this adapter.
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
        require_asset(&env, &asset)?;
        let next_report_nonce = next_report_nonce(&env, &asset)?;

        let reported = load_reported_assets(&env, &asset);
        let next = reported
            .checked_add(amount)
            .ok_or(AdapterError::ArithmeticOverflow)?;
        let adapter = env.current_contract_address();
        let custodian = get_custodian(&env)?;
        let token = soroban_sdk::token::Client::new(&env, &asset);
        transfer_exact(&token, &adapter, &custodian, amount)?;

        store_reported_assets(&env, &asset, next);
        store_report_nonce(&env, &asset, next_report_nonce);

        env.events()
            .publish((symbol_short!("supply"), asset, custodian), amount);
        Ok(())
    }

    /// Withdraw exactly `amount` of returned idle liquidity to the vault.
    ///
    /// This compatibility method is exact-only because it does not return the
    /// realized amount. Use `progress_withdrawal` when partial settlement is
    /// acceptable and the caller can account for the amount actually released.
    /// Settlement remains available while paused so already returned liquidity
    /// can be recovered during an incident.
    #[allow(deprecated)]
    pub fn withdraw(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_vault(&env, &caller)?;
        require_positive(amount)?;
        require_asset(&env, &asset)?;
        let next_report_nonce = next_report_nonce(&env, &asset)?;

        let adapter = env.current_contract_address();
        let token = soroban_sdk::token::Client::new(&env, &asset);
        let idle_balance = token.balance(&adapter);
        let reported = load_reported_assets(&env, &asset);
        let next_reported = exact_withdrawal_result(reported, idle_balance, amount)?;
        let vault = get_vault(&env)?;
        transfer_exact(&token, &adapter, &vault, amount)?;
        store_reported_assets(&env, &asset, next_reported);
        store_report_nonce(&env, &asset, next_report_nonce);

        env.events()
            .publish((symbol_short!("withdraw"), asset), amount);
        Ok(())
    }

    /// Progress a vault withdrawal using only assets already returned to this
    /// adapter. This method does not initiate any offchain market exit, and
    /// remains available while paused so returned liquidity can be recovered.
    #[allow(deprecated)]
    pub fn progress_withdrawal(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        require_vault(&env, &caller)?;
        require_positive(amount)?;
        require_asset(&env, &asset)?;
        let next_report_nonce = next_report_nonce(&env, &asset)?;

        let adapter = env.current_contract_address();
        let token = soroban_sdk::token::Client::new(&env, &asset);
        let idle_balance = token.balance(&adapter);
        let reported = load_reported_assets(&env, &asset);
        let (actual, next_reported) = withdrawal_result(reported, idle_balance, amount)?;
        let vault = get_vault(&env)?;
        transfer_exact(&token, &adapter, &vault, actual)?;
        store_reported_assets(&env, &asset, next_reported);
        store_report_nonce(&env, &asset, next_report_nonce);

        env.events()
            .publish((symbol_short!("withdraw"), asset), actual);
        Ok(actual)
    }

    /// Return the adapter's reported route NAV for `asset`.
    ///
    /// Returned idle balance is intentionally not auto-added here because
    /// reported route NAV already includes liquidity that has returned to this
    /// adapter but has not yet been released to the vault. Operators should use
    /// `set_reported_assets` for explicit NAV updates when offchain route value
    /// changes, and should avoid reporting only the offchain remainder while
    /// returned liquidity is still pending on this adapter.
    pub fn total_assets(env: Env, asset: Address) -> i128 {
        extend_instance_ttl(&env);
        require_asset(&env, &asset).unwrap_or_else(|err| panic_with_error!(&env, err));
        load_reported_assets(&env, &asset)
    }

    /// Explicitly set reported route NAV for `asset`.
    ///
    /// The configured custodian is allowed to report NAV because custody of the
    /// offchain position is already part of this adapter's trust boundary. This
    /// keeps reporting usable when the admin is a governance contract. The
    /// amount must represent total route NAV not yet released to the vault, not
    /// only the offchain remainder when returned idle liquidity is pending.
    /// `report_nonce` must be exactly one greater than the current report
    /// nonce; every successful NAV mutation advances this same revision.
    ///
    /// Pause blocks vault and custodian reporting, but intentionally leaves an
    /// admin recovery path for emergency NAV correction.
    #[allow(deprecated)]
    pub fn set_reported_assets(
        env: Env,
        caller: Address,
        asset: Address,
        expected_current: i128,
        amount: i128,
        report_nonce: u64,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_report_update_authority(&env, &caller)?;
        require_asset(&env, &asset)?;
        if expected_current < 0 || amount < 0 {
            return Err(AdapterError::InvalidInput);
        }
        if load_reported_assets(&env, &asset) != expected_current {
            return Err(AdapterError::InvalidInput);
        }
        let next_report_nonce = next_report_nonce(&env, &asset)?;
        if report_nonce != next_report_nonce {
            return Err(AdapterError::InvalidInput);
        }
        store_reported_assets(&env, &asset, amount);
        store_report_nonce(&env, &asset, report_nonce);
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

    pub fn asset(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_asset(&env)
    }

    pub fn report_nonce(env: Env, asset: Address) -> Result<u64, AdapterError> {
        extend_instance_ttl(&env);
        require_asset(&env, &asset)?;
        Ok(load_report_nonce(&env, &asset))
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

    /// Extend instance storage TTL.
    ///
    /// This is intentionally open: extending TTL is a liveness operation, and
    /// the transaction caller pays the Soroban resource cost.
    pub fn extend_ttl(env: Env) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
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

fn exact_withdrawal_result(
    reported: i128,
    idle_balance: i128,
    requested: i128,
) -> Result<i128, AdapterError> {
    require_positive(requested)?;
    if reported < 0 || idle_balance < 0 {
        return Err(AdapterError::InvalidInput);
    }
    if reported < requested || idle_balance < requested {
        return Err(AdapterError::InsufficientReturnedLiquidity);
    }
    reported
        .checked_sub(requested)
        .ok_or(AdapterError::ArithmeticUnderflow)
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

fn transfer_exact(
    token: &soroban_sdk::token::Client<'_>,
    from: &Address,
    to: &Address,
    amount: i128,
) -> Result<(), AdapterError> {
    let from_before = token.balance(from);
    let to_before = token.balance(to);
    token.transfer(from, to, &amount);
    let from_after = token.balance(from);
    let to_after = token.balance(to);
    let debited = from_before
        .checked_sub(from_after)
        .ok_or(AdapterError::InvalidInput)?;
    let credited = to_after
        .checked_sub(to_before)
        .ok_or(AdapterError::InvalidInput)?;
    if debited != amount || credited != amount {
        return Err(AdapterError::InvalidInput);
    }
    Ok(())
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

fn get_asset(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Asset)
}

fn require_asset(env: &Env, asset: &Address) -> Result<(), AdapterError> {
    let configured = get_asset(env)?;
    if asset != &configured {
        return Err(AdapterError::InvalidInput);
    }
    Ok(())
}

fn load_reported_assets(env: &Env, asset: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::ReportedAssets(asset.clone()))
        .unwrap_or(0)
}

fn load_report_nonce(env: &Env, asset: &Address) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::ReportNonce(asset.clone()))
        .unwrap_or(0)
}

fn next_report_nonce(env: &Env, asset: &Address) -> Result<u64, AdapterError> {
    load_report_nonce(env, asset)
        .checked_add(1)
        .ok_or(AdapterError::ArithmeticOverflow)
}

fn store_reported_assets(env: &Env, asset: &Address, amount: i128) {
    let key = DataKey::ReportedAssets(asset.clone());
    if amount == 0 {
        env.storage().instance().remove(&key);
    } else {
        env.storage().instance().set(&key, &amount);
    }
}

fn store_report_nonce(env: &Env, asset: &Address, nonce: u64) {
    env.storage()
        .instance()
        .set(&DataKey::ReportNonce(asset.clone()), &nonce);
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

fn require_report_update_authority(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let admin = get_admin(env)?;
    if caller == &admin {
        return Ok(());
    }
    if is_paused(env) {
        return Err(AdapterError::Paused);
    }
    let vault = get_vault(env)?;
    let custodian = get_custodian(env)?;
    if caller != &vault && caller != &custodian {
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

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn withdrawal_result_conserves_reported_assets() {
        let reported: i128 = kani::any();
        let idle: i128 = kani::any();
        let requested: i128 = kani::any();
        let result = withdrawal_result(reported, idle, requested);

        if requested <= 0 || reported < 0 || idle < 0 {
            assert_eq!(result, Err(AdapterError::InvalidInput));
        } else {
            let expected_actual = min_i128(min_i128(reported, idle), requested);
            if expected_actual == 0 {
                assert_eq!(result, Err(AdapterError::InsufficientReturnedLiquidity));
            } else {
                let (actual, next_reported) = result.unwrap();
                assert_eq!(actual, expected_actual);
                assert!(actual > 0);
                assert!(actual <= reported);
                assert!(actual <= idle);
                assert!(actual <= requested);
                assert_eq!(next_reported, reported - actual);
                assert_eq!(actual + next_reported, reported);
                assert!(next_reported >= 0);
            }
        }
    }
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

    #[contract]
    struct FeeToken;

    #[contractimpl]
    impl FeeToken {
        pub fn mint(env: Env, to: Address, amount: i128) {
            let current = Self::balance(env.clone(), to.clone());
            env.storage()
                .instance()
                .set(&DataKey::ReportedAssets(to), &(current + amount));
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            env.storage()
                .instance()
                .get(&DataKey::ReportedAssets(id))
                .unwrap_or(0)
        }

        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            let from_balance = Self::balance(env.clone(), from.clone());
            let to_balance = Self::balance(env.clone(), to.clone());
            let credited = amount.saturating_sub(1);
            env.storage()
                .instance()
                .set(&DataKey::ReportedAssets(from), &(from_balance - amount));
            env.storage()
                .instance()
                .set(&DataKey::ReportedAssets(to), &(to_balance + credited));
        }
    }

    fn register_dummy_contract(env: &Env) -> Address {
        env.register(DummyContract, ())
    }

    fn register_fee_token(env: &Env) -> Address {
        env.register(FeeToken, ())
    }

    fn account_address(env: &Env) -> Address {
        Address::from_str(
            env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
    }

    fn setup_adapter(env: &Env) -> (Address, Address, Address, Address, Address) {
        let admin = account_address(env);
        let vault = register_dummy_contract(env);
        let custodian = Address::generate(env);
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(env));
        let asset = asset_sac.address();
        let contract_id = setup_adapter_with(env, &admin, &vault, &custodian, &asset);
        (contract_id, admin, vault, custodian, asset)
    }

    fn setup_adapter_with(
        env: &Env,
        admin: &Address,
        vault: &Address,
        custodian: &Address,
        asset: &Address,
    ) -> Address {
        env.register(CustodialAdapterContract, (admin, vault, custodian, asset))
    }

    fn token_balance(env: &Env, token: &Address, account: &Address) -> i128 {
        soroban_sdk::token::Client::new(env, token).balance(account)
    }

    fn reported_assets(env: &Env, contract_id: &Address, asset: &Address) -> i128 {
        env.as_contract(contract_id, || {
            CustodialAdapterContract::total_assets(env.clone(), asset.clone())
        })
    }

    fn report_nonce(env: &Env, contract_id: &Address, asset: &Address) -> u64 {
        env.as_contract(contract_id, || {
            CustodialAdapterContract::report_nonce(env.clone(), asset.clone()).unwrap()
        })
    }

    fn has_reported_assets_key(env: &Env, contract_id: &Address, asset: &Address) -> bool {
        env.as_contract(contract_id, || {
            env.storage()
                .instance()
                .has(&DataKey::ReportedAssets(asset.clone()))
        })
    }

    fn set_reported_assets_for(
        env: &Env,
        contract_id: &Address,
        caller: Address,
        asset: Address,
        amount: i128,
    ) {
        let expected_current = reported_assets(env, contract_id, &asset);
        let report_nonce = report_nonce(env, contract_id, &asset) + 1;
        env.as_contract(contract_id, || {
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                caller,
                asset,
                expected_current,
                amount,
                report_nonce,
            )
            .unwrap();
        });
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
        let (contract_id, admin, vault, custodian, asset) = setup_adapter(&env);
        env.as_contract(&contract_id, || {
            assert_eq!(CustodialAdapterContract::admin(env.clone()).unwrap(), admin);
            assert_eq!(CustodialAdapterContract::vault(env.clone()).unwrap(), vault);
            assert_eq!(
                CustodialAdapterContract::custodian(env.clone()).unwrap(),
                custodian
            );
            assert_eq!(CustodialAdapterContract::asset(env.clone()).unwrap(), asset);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_non_contract_vault() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = account_address(&env);
        let custodian = Address::generate(&env);
        let asset = register_dummy_contract(&env);
        env.register(
            CustodialAdapterContract,
            (&admin, &vault, &custodian, &asset),
        );
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_non_contract_asset() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = register_dummy_contract(&env);
        let custodian = Address::generate(&env);
        let asset = account_address(&env);
        env.register(
            CustodialAdapterContract,
            (&admin, &vault, &custodian, &asset),
        );
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_custodian_equal_to_vault() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = register_dummy_contract(&env);
        let asset = register_dummy_contract(&env);
        env.register(CustodialAdapterContract, (&admin, &vault, &vault, &asset));
    }

    #[test]
    fn constructor_rejects_custodian_equal_to_adapter() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = register_dummy_contract(&env);
        let adapter = Address::generate(&env);
        let asset = register_dummy_contract(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.register_at(
                &adapter,
                CustodialAdapterContract,
                (&admin, &vault, &adapter, &asset),
            );
        }));

        assert!(result.is_err());
    }

    #[test]
    fn constructor_rejects_admin_equal_to_adapter() {
        let env = Env::default();
        let adapter = Address::generate(&env);
        let vault = register_dummy_contract(&env);
        let custodian = Address::generate(&env);
        let asset = register_dummy_contract(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.register_at(
                &adapter,
                CustodialAdapterContract,
                (&adapter, &vault, &custodian, &asset),
            );
        }));

        assert!(result.is_err());
    }

    #[test]
    fn constructor_rejects_vault_equal_to_adapter() {
        let env = Env::default();
        let admin = account_address(&env);
        let adapter = Address::generate(&env);
        let custodian = Address::generate(&env);
        let asset = register_dummy_contract(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.register_at(
                &adapter,
                CustodialAdapterContract,
                (&admin, &adapter, &custodian, &asset),
            );
        }));

        assert!(result.is_err());
    }

    #[test]
    fn constructor_rejects_asset_equal_to_adapter() {
        let env = Env::default();
        let admin = account_address(&env);
        let adapter = Address::generate(&env);
        let vault = register_dummy_contract(&env);
        let custodian = Address::generate(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.register_at(
                &adapter,
                CustodialAdapterContract,
                (&admin, &vault, &custodian, &adapter),
            );
        }));

        assert!(result.is_err());
    }

    #[test]
    fn constructor_rejects_asset_equal_to_role_addresses() {
        let env = Env::default();
        let admin = register_dummy_contract(&env);
        let vault = register_dummy_contract(&env);
        let custodian = register_dummy_contract(&env);
        let asset = register_dummy_contract(&env);

        for invalid_asset in [&admin, &vault, &custodian] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                env.register(
                    CustodialAdapterContract,
                    (&admin, &vault, &custodian, invalid_asset),
                );
            }));
            assert!(result.is_err());
        }

        env.register(
            CustodialAdapterContract,
            (&admin, &vault, &custodian, &asset),
        );
    }

    #[test]
    fn supply_forwards_funds_and_reports_assets() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
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
        let (contract_id, _admin, _vault, _custodian, asset) = setup_adapter(&env);
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
        let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
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
        let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
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
        assert_eq!(reported_assets(&env, &contract_id, &asset), 1_000);

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
    fn withdraw_requires_exact_returned_liquidity() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1_000);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 1_000)
                .unwrap();
        });
        asset_admin.mint(&custodian, &400);
        soroban_sdk::token::Client::new(&env, &asset)
            .mock_all_auths()
            .transfer(&custodian, &contract_id, &400);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 700),
                Err(AdapterError::InsufficientReturnedLiquidity)
            );
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 1_000);
        assert_eq!(token_balance(&env, &asset, &contract_id), 400);
        assert_eq!(token_balance(&env, &asset, &vault), 0);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 400)
                .unwrap();
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 600);
        assert_eq!(token_balance(&env, &asset, &contract_id), 0);
        assert_eq!(token_balance(&env, &asset, &vault), 400);
    }

    #[test]
    fn withdrawal_requires_configured_vault() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _custodian, asset) = setup_adapter(&env);
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
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &500);
        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 500);

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
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);

        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 1_000);
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
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1_000);

        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 250);
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
    fn nav_mutations_advance_report_nonce_exactly_once() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &100);
        assert_eq!(report_nonce(&env, &contract_id, &asset), 0);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 100)
                .unwrap();
        });
        assert_eq!(report_nonce(&env, &contract_id, &asset), 1);

        asset_admin.mint(&custodian, &50);
        soroban_sdk::token::Client::new(&env, &asset)
            .mock_all_auths()
            .transfer(&custodian, &contract_id, &50);
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    20,
                )
                .unwrap(),
                20
            );
        });
        assert_eq!(report_nonce(&env, &contract_id, &asset), 2);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 10)
                .unwrap();
        });
        assert_eq!(report_nonce(&env, &contract_id, &asset), 3);

        set_reported_assets_for(&env, &contract_id, vault, asset.clone(), 80);
        assert_eq!(report_nonce(&env, &contract_id, &asset), 4);
    }

    #[test]
    fn set_reported_assets_requires_admin_vault_or_custodian() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, custodian, asset) = setup_adapter(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    impostor,
                    asset.clone(),
                    0,
                    1,
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                admin,
                asset.clone(),
                0,
                9,
                1,
            )
            .unwrap();
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                vault,
                asset.clone(),
                9,
                7,
                2,
            )
            .unwrap();
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                custodian,
                asset.clone(),
                7,
                5,
                3,
            )
            .unwrap();
            assert_eq!(
                CustodialAdapterContract::total_assets(env.clone(), asset.clone()),
                5
            );
        });
    }

    #[test]
    fn zero_reported_assets_removes_storage_key() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);

        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 7);
        assert_eq!(reported_assets(&env, &contract_id, &asset), 7);
        assert!(has_reported_assets_key(&env, &contract_id, &asset));

        set_reported_assets_for(&env, &contract_id, vault, asset.clone(), 0);
        assert_eq!(reported_assets(&env, &contract_id, &asset), 0);
        assert!(!has_reported_assets_key(&env, &contract_id, &asset));
    }

    #[test]
    fn negative_or_zero_amounts_are_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
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
                CustodialAdapterContract::set_reported_assets(env.clone(), vault, asset, 0, -1, 1),
                Err(AdapterError::InvalidInput)
            );
        });
    }

    #[test]
    fn failed_operations_leave_reported_assets_unchanged() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, _custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &10);
        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 100);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(
                    env.clone(),
                    Address::generate(&env),
                    asset.clone(),
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    Address::generate(&env),
                    asset.clone(),
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    Address::generate(&env),
                    asset.clone(),
                    100,
                    1,
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 100);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 0),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), -1),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    100,
                    -1,
                    2
                ),
                Err(AdapterError::InvalidInput)
            );
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 100);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_paused(env.clone(), admin.clone(), true).unwrap();
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 1),
                Err(AdapterError::Paused)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    100,
                    1,
                    2
                ),
                Err(AdapterError::Paused)
            );
        });
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_paused(env.clone(), admin.clone(), false).unwrap();
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 100);
    }

    #[test]
    fn supply_overflow_fails_before_transfer_or_accounting_change() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &1);
        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), i128::MAX);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), vault, asset.clone(), 1),
                Err(AdapterError::ArithmeticOverflow)
            );
        });

        assert_eq!(reported_assets(&env, &contract_id, &asset), i128::MAX);
        assert_eq!(token_balance(&env, &asset, &contract_id), 1);
        assert_eq!(token_balance(&env, &asset, &custodian), 0);
    }

    #[test]
    fn withdrawal_accounting_matches_transferred_amount_for_each_bound() {
        for (reported, idle, requested, expected_actual) in [
            (1_000, 600, 400, 400),
            (300, 600, 400, 300),
            (1_000, 250, 400, 250),
        ] {
            let env = Env::default();
            env.mock_all_auths();
            let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
            let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
            asset_admin.mint(&contract_id, &idle);
            set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), reported);

            let actual = env.as_contract(&contract_id, || {
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    requested,
                )
                .unwrap()
            });

            assert_eq!(actual, expected_actual);
            assert_eq!(
                reported_assets(&env, &contract_id, &asset),
                reported - expected_actual
            );
            assert_eq!(token_balance(&env, &asset, &vault), expected_actual);
            assert_eq!(
                token_balance(&env, &asset, &contract_id),
                idle - expected_actual
            );
        }
    }

    #[test]
    fn unsupported_assets_are_rejected_without_accounting_change() {
        {
            let env = Env::default();
            env.mock_all_auths();
            let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
            let unsupported_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
            let unsupported = unsupported_sac.address();
            set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 10);
            env.as_contract(&contract_id, || {
                assert_eq!(
                    CustodialAdapterContract::set_reported_assets(
                        env.clone(),
                        vault.clone(),
                        unsupported.clone(),
                        0,
                        200,
                        1,
                    ),
                    Err(AdapterError::InvalidInput)
                );
            });
            assert_eq!(reported_assets(&env, &contract_id, &asset), 10);
            assert!(!has_reported_assets_key(&env, &contract_id, &unsupported));
        }

        {
            let env = Env::default();
            env.mock_all_auths();
            let (contract_id, _admin, vault, custodian, asset) = setup_adapter(&env);
            let unsupported_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
            let unsupported = unsupported_sac.address();
            soroban_sdk::token::StellarAssetClient::new(&env, &asset).mint(&contract_id, &100);
            set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 10);
            env.as_contract(&contract_id, || {
                assert_eq!(
                    CustodialAdapterContract::supply(
                        env.clone(),
                        vault.clone(),
                        unsupported.clone(),
                        40
                    ),
                    Err(AdapterError::InvalidInput)
                );
            });
            assert_eq!(reported_assets(&env, &contract_id, &asset), 10);
            assert!(!has_reported_assets_key(&env, &contract_id, &unsupported));
            assert_eq!(token_balance(&env, &unsupported, &custodian), 0);
        }

        {
            let env = Env::default();
            env.mock_all_auths();
            let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
            let unsupported_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
            let unsupported = unsupported_sac.address();
            set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 10);
            env.as_contract(&contract_id, || {
                assert_eq!(
                    CustodialAdapterContract::progress_withdrawal(
                        env.clone(),
                        vault.clone(),
                        unsupported.clone(),
                        25
                    ),
                    Err(AdapterError::InvalidInput)
                );
            });
            assert_eq!(reported_assets(&env, &contract_id, &asset), 10);
            assert!(!has_reported_assets_key(&env, &contract_id, &unsupported));
            assert_eq!(token_balance(&env, &unsupported, &vault), 0);
        }

        {
            let env = Env::default();
            env.mock_all_auths();
            let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
            let unsupported_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
            let unsupported = unsupported_sac.address();
            set_reported_assets_for(&env, &contract_id, vault, asset.clone(), 10);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                env.as_contract(&contract_id, || {
                    CustodialAdapterContract::total_assets(env.clone(), unsupported.clone());
                });
            }));
            assert!(result.is_err());
            assert_eq!(reported_assets(&env, &contract_id, &asset), 10);
            assert!(!has_reported_assets_key(&env, &contract_id, &unsupported));
        }
    }

    #[test]
    fn fee_on_transfer_asset_is_rejected_without_accounting_change() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = account_address(&env);
        let vault = register_dummy_contract(&env);
        let custodian = Address::generate(&env);
        let asset = register_fee_token(&env);
        let contract_id = setup_adapter_with(&env, &admin, &vault, &custodian, &asset);

        env.as_contract(&asset, || {
            FeeToken::mint(env.clone(), contract_id.clone(), 100);
        });
        assert_eq!(token_balance(&env, &asset, &contract_id), 100);
        assert_eq!(token_balance(&env, &asset, &custodian), 0);

        let args: soroban_sdk::Vec<soroban_sdk::Val> = (&vault, &asset, &40i128).into_val(&env);
        let result = env.try_invoke_contract::<(), AdapterError>(
            &contract_id,
            &Symbol::new(&env, "supply"),
            args,
        );
        assert_eq!(result, Err(Ok(AdapterError::InvalidInput)));
        assert_eq!(reported_assets(&env, &contract_id, &asset), 0);
        assert_eq!(token_balance(&env, &asset, &contract_id), 100);
        assert_eq!(token_balance(&env, &asset, &custodian), 0);
        assert_eq!(report_nonce(&env, &contract_id, &asset), 0);
    }

    #[test]
    fn total_assets_is_reported_nav_not_idle_balance() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);

        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 100);
        asset_admin.mint(&contract_id, &50);
        assert_eq!(token_balance(&env, &asset, &contract_id), 50);
        assert_eq!(reported_assets(&env, &contract_id, &asset), 100);

        env.as_contract(&contract_id, || {
            let actual = CustodialAdapterContract::progress_withdrawal(
                env.clone(),
                vault.clone(),
                asset.clone(),
                40,
            )
            .unwrap();
            assert_eq!(actual, 40);
        });
        assert_eq!(token_balance(&env, &asset, &contract_id), 10);
        assert_eq!(reported_assets(&env, &contract_id, &asset), 60);
    }

    #[test]
    fn pause_blocks_new_exposure_but_allows_returned_liquidity_settlement() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &50);
        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 50);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_paused(env.clone(), admin.clone(), true).unwrap();
            assert!(CustodialAdapterContract::paused(env.clone()));
            assert_eq!(
                CustodialAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 1),
                Err(AdapterError::Paused)
            );
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    50,
                    60,
                    2,
                ),
                Err(AdapterError::Paused)
            );
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    custodian.clone(),
                    asset.clone(),
                    50,
                    60,
                    2,
                ),
                Err(AdapterError::Paused)
            );
        });

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    20
                )
                .unwrap(),
                20
            );
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 30);
        assert_eq!(token_balance(&env, &asset, &vault), 20);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 10)
                .unwrap();
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 20);
        assert_eq!(token_balance(&env, &asset, &vault), 30);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    20,
                    30,
                    2,
                ),
                Err(AdapterError::Paused)
            );
        });
        set_reported_assets_for(&env, &contract_id, admin, asset.clone(), 30);
        assert_eq!(reported_assets(&env, &contract_id, &asset), 30);
    }

    #[test]
    fn stale_or_replayed_absolute_reports_are_rejected_after_withdrawal() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _custodian, asset) = setup_adapter(&env);
        let asset_admin = soroban_sdk::token::StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&contract_id, &400);
        set_reported_assets_for(&env, &contract_id, vault.clone(), asset.clone(), 1_000);
        assert_eq!(report_nonce(&env, &contract_id, &asset), 1);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    400,
                )
                .unwrap(),
                400
            );
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 600);
        assert_eq!(report_nonce(&env, &contract_id, &asset), 2);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    1_000,
                    1_000,
                    2,
                ),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    600,
                    650,
                    1,
                ),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    vault.clone(),
                    asset.clone(),
                    600,
                    650,
                    u64::MAX,
                ),
                Err(AdapterError::InvalidInput)
            );
        });
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_reported_assets(
                env.clone(),
                vault.clone(),
                asset.clone(),
                600,
                650,
                3,
            )
            .unwrap();
        });
        assert_eq!(reported_assets(&env, &contract_id, &asset), 650);
        assert_eq!(report_nonce(&env, &contract_id, &asset), 3);
    }

    #[test]
    fn pause_does_not_block_admin_recovery_or_ttl_extension() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _custodian, _asset) = setup_adapter(&env);
        let new_admin = Address::generate(&env);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_paused(env.clone(), admin.clone(), true).unwrap();
        });
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_admin(env.clone(), admin.clone(), new_admin.clone())
                .unwrap();
            assert_eq!(CustodialAdapterContract::admin(env.clone()).unwrap(), admin);
        });
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::accept_admin(env.clone(), new_admin.clone()).unwrap();
            assert_eq!(
                CustodialAdapterContract::admin(env.clone()).unwrap(),
                new_admin
            );
        });
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::extend_ttl(env.clone()).unwrap();
        });
    }

    #[test]
    fn authorization_matrix_rejects_untrusted_callers() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, _custodian, asset) = setup_adapter(&env);
        let impostor = Address::generate(&env);
        let new_admin = Address::generate(&env);

        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_paused(env.clone(), impostor.clone(), true),
                Err(AdapterError::Unauthorized)
            );
            CustodialAdapterContract::set_paused(env.clone(), admin.clone(), true).unwrap();
            CustodialAdapterContract::set_paused(env.clone(), vault.clone(), false).unwrap();
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::supply(
                    env.clone(),
                    Address::generate(&env),
                    asset.clone(),
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::progress_withdrawal(
                    env.clone(),
                    Address::generate(&env),
                    asset.clone(),
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_reported_assets(
                    env.clone(),
                    Address::generate(&env),
                    asset.clone(),
                    0,
                    1,
                    1
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::set_admin(
                    env.clone(),
                    Address::generate(&env),
                    new_admin.clone()
                ),
                Err(AdapterError::Unauthorized)
            );
        });
        env.as_contract(&contract_id, || {
            CustodialAdapterContract::set_admin(env.clone(), admin, new_admin.clone()).unwrap();
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                CustodialAdapterContract::accept_admin(env.clone(), impostor),
                Err(AdapterError::Unauthorized)
            );
            assert_eq!(
                CustodialAdapterContract::admin(env.clone()).unwrap(),
                account_address(&env)
            );
        });
        env.as_contract(&contract_id, || {
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
    fn upgrade_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _custodian, _asset) = setup_adapter(&env);
        let impostor = Address::generate(&env);
        let new_hash = empty_wasm_hash(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.as_contract(&contract_id, || {
                CustodialAdapterContract::upgrade(&env, new_hash, impostor);
            });
        }));

        assert!(result.is_err());
    }

    #[test]
    fn set_admin_requires_pending_admin_acceptance() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _custodian, _asset) = setup_adapter(&env);
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
    fn extend_ttl_is_open_liveness_maintenance() {
        let env = Env::default();
        env.mock_auths(&[]);
        let (contract_id, _admin, _vault, _custodian, _asset) = setup_adapter(&env);

        env.as_contract(&contract_id, || {
            CustodialAdapterContract::extend_ttl(env.clone()).unwrap();
        });
    }

    #[test]
    fn admin_events_and_upgrade_emit() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _custodian, _asset) = setup_adapter(&env);
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
        fn withdrawal_result_matches_conservation_model(
            reported in any::<i128>(),
            idle in any::<i128>(),
            requested in any::<i128>(),
        ) {
            let result = simulate_progress_withdrawal(reported, idle, requested);

            if requested <= 0 || reported < 0 || idle < 0 {
                prop_assert_eq!(result, Err(AdapterError::InvalidInput));
            } else if min_i128(min_i128(reported, idle), requested) == 0 {
                prop_assert_eq!(result, Err(AdapterError::InsufficientReturnedLiquidity));
            } else {
                let expected_actual = min_i128(min_i128(reported, idle), requested);
                let (actual, next_reported) =
                    result.expect("positive reported, idle, and request should withdraw");
                prop_assert_eq!(actual, expected_actual);
                prop_assert!(actual > 0);
                prop_assert!(actual <= reported);
                prop_assert!(actual <= idle);
                prop_assert!(actual <= requested);
                prop_assert_eq!(next_reported, reported - actual);
                prop_assert_eq!(actual + next_reported, reported);
                prop_assert!(next_reported >= 0);
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
