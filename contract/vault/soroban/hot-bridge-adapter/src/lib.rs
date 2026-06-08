#![no_std]

use soroban_sdk::{
    address_payload::AddressPayload,
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, IntoVal, Symbol, Val, Vec,
};
use stellar_contract_utils::upgradeable::{self, Upgradeable};

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
const HOT_TIMESTAMP_SCALE: u128 = 1_000_000_000_000;
const STORAGE_VERSION: u32 = 1;
const ADMIN_TRANSFER_TTL_LEDGERS: u32 = 172_800;

#[contracttype]
#[derive(Clone, Debug)]
enum DataKey {
    StorageVersion,
    Admin,
    PendingAdminTransfer,
    Vault,
    HotLocker,
    HotReceiverId,
    Asset,
    OperationalState,
    LastHotClientTimestamp,
    Principal(Address),
    Returned(Address),
}

#[contracttype]
#[derive(Clone, Debug)]
struct PendingAdminTransfer {
    candidate: Address,
    proposed_by: Address,
    proposed_at: u32,
    expires_at: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
enum OperationalState {
    Active,
    Paused(Address, u32),
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
    InsufficientPrincipal = 6,
    InsufficientReturnedBalance = 7,
    Paused = 8,
    InsufficientAdapterBalance = 9,
    DepositPostconditionFailed = 10,
    HotClientTimestampExhausted = 11,
    UnsupportedAsset = 12,
    PendingReturnedBalance = 13,
    InsufficientRecordedReturn = 14,
    InsufficientSurplus = 15,
    PendingAdminExpired = 16,
}

#[contract]
pub struct HotBridgeAdapterContract;

#[contractimpl]
#[allow(deprecated)]
impl HotBridgeAdapterContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        hot_locker: Address,
        asset: Address,
        hot_receiver_id: BytesN<32>,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_contract_address(&admin, AdapterError::InvalidInput)?;
        require_contract_address(&vault, AdapterError::InvalidInput)?;
        require_contract_address(&hot_locker, AdapterError::InvalidInput)?;
        require_contract_address(&asset, AdapterError::InvalidInput)?;

        env.storage()
            .instance()
            .set(&DataKey::StorageVersion, &STORAGE_VERSION);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage()
            .instance()
            .set(&DataKey::HotLocker, &hot_locker);
        env.storage().instance().set(&DataKey::Asset, &asset);
        env.storage()
            .instance()
            .set(&DataKey::HotReceiverId, &hot_receiver_id);
        Ok(())
    }

    pub fn set_paused(env: Env, caller: Address, paused: bool) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin_or_vault(&env, &caller)?;
        let state = if paused {
            OperationalState::Paused(caller.clone(), env.ledger().sequence())
        } else {
            OperationalState::Active
        };
        env.storage()
            .instance()
            .set(&DataKey::OperationalState, &state);
        env.events()
            .publish((symbol_short!("paused"), caller), paused);
        Ok(())
    }

    pub fn paused(env: Env) -> bool {
        extend_instance_ttl(&env);
        is_paused(&env)
    }

    pub fn supply(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        require_positive_amount(amount)?;
        require_configured_asset(&env, &asset)?;
        // Returned balances must be progressed back to the vault before new supply
        // can bridge more of the same asset, otherwise principal and completed
        // withdrawal accounting would be mixed.
        if returned_of(&env, &asset) > 0 {
            return Err(AdapterError::PendingReturnedBalance);
        }

        let adapter = env.current_contract_address();
        let hot_locker = get_hot_locker(&env)?;
        let hot_receiver_id = get_hot_receiver_id(&env)?;
        let client_timestamp = next_hot_locker_timestamp(&env)?;
        let amount_u128 = amount_as_u128(amount)?;
        let token = soroban_sdk::token::Client::new(&env, &asset);
        if token.balance(&adapter) != 0 {
            return Err(AdapterError::InsufficientAdapterBalance);
        }
        let vault = get_vault(&env)?;
        token.transfer(&vault, &adapter, &amount);
        let balance_before = token.balance(&adapter);

        authorize_hot_deposit(&env, &hot_locker, &asset, &adapter, amount);

        let args: Vec<Val> = (
            adapter.clone(),
            amount_u128,
            asset.clone(),
            hot_receiver_id.clone(),
            client_timestamp,
        )
            .into_val(&env);
        let returned_nonce =
            env.invoke_contract::<u128>(&hot_locker, &Symbol::new(&env, "deposit"), args);

        let balance_after = token.balance(&adapter);
        if let Err(err) = validate_supply_balance_change(balance_before, balance_after, amount) {
            let leftover = token.balance(&adapter);
            if leftover > 0 {
                token.transfer(&adapter, &vault, &leftover);
            }
            return Err(err);
        }

        record_hot_locker_timestamp(&env, client_timestamp);
        increase_principal(&env, &asset, amount)?;
        env.events().publish(
            (symbol_short!("hot_dep"), asset.clone()),
            (amount, hot_locker, hot_receiver_id, returned_nonce),
        );
        env.events()
            .publish((symbol_short!("supply"), asset), amount);
        Ok(())
    }

    pub fn withdraw(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        Self::progress_withdrawal(env, caller, asset, amount).map(|_| ())
    }

    pub fn progress_withdrawal(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        require_positive_amount(amount)?;
        require_configured_asset(&env, &asset)?;

        let principal = principal_of(&env, &asset);
        if principal < amount {
            return Err(AdapterError::InsufficientPrincipal);
        }

        let adapter = env.current_contract_address();
        let vault = get_vault(&env)?;
        let token = soroban_sdk::token::Client::new(&env, &asset);
        validate_withdrawal_resources(
            principal,
            returned_of(&env, &asset),
            token.balance(&adapter),
            amount,
        )?;

        token.transfer(&adapter, &vault, &amount);
        decrease_returned(&env, &asset, amount)?;
        decrease_principal(&env, &asset, amount)?;
        env.events()
            .publish((symbol_short!("withdraw"), asset), amount);
        Ok(amount)
    }

    pub fn total_assets(env: Env, asset: Address) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        require_configured_asset(&env, &asset)?;
        Ok(principal_of(&env, &asset))
    }

    pub fn principal(env: Env, asset: Address) -> i128 {
        extend_instance_ttl(&env);
        principal_of(&env, &asset)
    }

    pub fn returned_balance(env: Env, asset: Address) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        require_configured_asset(&env, &asset)?;
        Ok(returned_of(&env, &asset))
    }

    pub fn hot_receiver_id(env: Env) -> Result<BytesN<32>, AdapterError> {
        extend_instance_ttl(&env);
        get_hot_receiver_id(&env)
    }

    pub fn hot_locker(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_hot_locker(&env)
    }

    pub fn admin(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_admin(&env)
    }

    pub fn vault(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_vault(&env)
    }

    pub fn asset(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_configured_asset(&env)
    }

    pub fn storage_version(env: Env) -> u32 {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get(&DataKey::StorageVersion)
            .unwrap_or(STORAGE_VERSION)
    }

    pub fn set_asset(env: Env, caller: Address, asset: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_contract_address(&asset, AdapterError::InvalidInput)?;
        if let Ok(current) = get_configured_asset(&env) {
            if current != asset {
                let has_position =
                    principal_of(&env, &current) > 0 || returned_of(&env, &current) > 0;
                if has_position {
                    return Err(AdapterError::InvalidInput);
                }
            }
        }
        env.storage().instance().set(&DataKey::Asset, &asset);
        env.events()
            .publish((symbol_short!("asset"), caller), asset);
        Ok(())
    }

    pub fn propose_admin(
        env: Env,
        caller: Address,
        new_admin: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_contract_address(&new_admin, AdapterError::InvalidInput)?;
        let transfer = PendingAdminTransfer {
            candidate: new_admin.clone(),
            proposed_by: caller.clone(),
            proposed_at: env.ledger().sequence(),
            expires_at: env
                .ledger()
                .sequence()
                .saturating_add(ADMIN_TRANSFER_TTL_LEDGERS),
        };
        env.storage()
            .instance()
            .set(&DataKey::PendingAdminTransfer, &transfer);
        env.events()
            .publish((symbol_short!("adm_prop"), caller), new_admin);
        Ok(())
    }

    pub fn accept_admin(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        caller.require_auth();
        let pending: PendingAdminTransfer = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdminTransfer)
            .ok_or(AdapterError::MissingConfig)?;
        if env.ledger().sequence() > pending.expires_at {
            return Err(AdapterError::PendingAdminExpired);
        }
        if caller != pending.candidate {
            return Err(AdapterError::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Admin, &caller);
        env.storage()
            .instance()
            .remove(&DataKey::PendingAdminTransfer);
        env.events().publish((symbol_short!("adm_acc"), caller), ());
        Ok(())
    }

    pub fn pending_admin(env: Env) -> Option<Address> {
        extend_instance_ttl(&env);
        env.storage()
            .instance()
            .get::<_, PendingAdminTransfer>(&DataKey::PendingAdminTransfer)
            .map(|transfer| transfer.candidate)
    }

    pub fn cancel_admin_transfer(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage()
            .instance()
            .remove(&DataKey::PendingAdminTransfer);
        env.events().publish((symbol_short!("adm_can"), caller), ());
        Ok(())
    }

    pub fn record_returned(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_hot_locker(&env, &caller)?;
        require_positive_amount(amount)?;
        require_configured_asset(&env, &asset)?;
        let principal = principal_of(&env, &asset);
        let returned = returned_of(&env, &asset);
        let updated = returned
            .checked_add(amount)
            .ok_or(AdapterError::ArithmeticOverflow)?;
        if updated > principal {
            return Err(AdapterError::InsufficientPrincipal);
        }
        env.storage()
            .instance()
            .set(&DataKey::Returned(asset.clone()), &updated);
        env.events()
            .publish((symbol_short!("returned"), asset), amount);
        Ok(())
    }

    pub fn rescue(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
        receiver: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        require_positive_amount(amount)?;
        require_configured_asset(&env, &asset)?;
        require_contract_address(&receiver, AdapterError::InvalidInput)?;
        if receiver == env.current_contract_address() {
            return Err(AdapterError::InvalidInput);
        }

        let adapter = env.current_contract_address();
        let token = soroban_sdk::token::Client::new(&env, &asset);
        let balance = token.balance(&adapter);
        let reserved = returned_of(&env, &asset);
        let surplus = balance
            .checked_sub(reserved)
            .ok_or(AdapterError::InsufficientSurplus)?;
        if amount > surplus {
            return Err(AdapterError::InsufficientSurplus);
        }
        token.transfer(&adapter, &receiver, &amount);
        env.events()
            .publish((symbol_short!("rescue"), asset, receiver), amount);
        Ok(())
    }
}

fn authorize_hot_deposit(
    env: &Env,
    hot_locker: &Address,
    asset: &Address,
    adapter: &Address,
    amount: i128,
) {
    env.authorize_as_current_contract(Vec::from_array(
        env,
        [InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: asset.clone(),
                fn_name: Symbol::new(env, "transfer"),
                args: (adapter.clone(), hot_locker.clone(), amount).into_val(env),
            },
            sub_invocations: Vec::new(env),
        })],
    ));
}

fn require_positive_amount(amount: i128) -> Result<(), AdapterError> {
    if amount <= 0 {
        return Err(AdapterError::InvalidInput);
    }
    Ok(())
}

fn amount_as_u128(amount: i128) -> Result<u128, AdapterError> {
    u128::try_from(amount).map_err(|_| AdapterError::InvalidInput)
}

fn hot_locker_timestamp(ledger_timestamp: u64) -> Result<u128, AdapterError> {
    u128::from(ledger_timestamp)
        .checked_mul(HOT_TIMESTAMP_SCALE)
        .ok_or(AdapterError::ArithmeticOverflow)
}

fn next_hot_locker_timestamp(env: &Env) -> Result<u128, AdapterError> {
    let base_timestamp = hot_locker_timestamp(env.ledger().timestamp())?;
    let last_timestamp = env
        .storage()
        .instance()
        .get::<_, u128>(&DataKey::LastHotClientTimestamp);
    choose_hot_client_timestamp(base_timestamp, last_timestamp)
}

fn choose_hot_client_timestamp(
    base_timestamp: u128,
    last_timestamp: Option<u128>,
) -> Result<u128, AdapterError> {
    let client_timestamp = if let Some(last_timestamp) = last_timestamp {
        if base_timestamp <= last_timestamp {
            last_timestamp
                .checked_sub(1)
                .ok_or(AdapterError::HotClientTimestampExhausted)?
        } else {
            base_timestamp
        }
    } else {
        base_timestamp
    };

    Ok(client_timestamp)
}

fn record_hot_locker_timestamp(env: &Env, client_timestamp: u128) {
    env.storage()
        .instance()
        .set(&DataKey::LastHotClientTimestamp, &client_timestamp);
}

fn validate_supply_balance_change(
    balance_before: i128,
    balance_after: i128,
    amount: i128,
) -> Result<(), AdapterError> {
    if balance_before < amount {
        return Err(AdapterError::InsufficientAdapterBalance);
    }

    let spent = balance_before
        .checked_sub(balance_after)
        .ok_or(AdapterError::DepositPostconditionFailed)?;
    if spent != amount {
        return Err(AdapterError::DepositPostconditionFailed);
    }

    Ok(())
}

fn validate_withdrawal_resources(
    principal: i128,
    recorded_returned: i128,
    adapter_balance: i128,
    amount: i128,
) -> Result<(), AdapterError> {
    if principal < amount {
        return Err(AdapterError::InsufficientPrincipal);
    }
    if recorded_returned < amount {
        return Err(AdapterError::InsufficientRecordedReturn);
    }
    if adapter_balance < amount {
        return Err(AdapterError::InsufficientReturnedBalance);
    }

    Ok(())
}

fn principal_of(env: &Env, asset: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::Principal(asset.clone()))
        .unwrap_or(0)
}

fn principal_after_increase(current: i128, amount: i128) -> Result<i128, AdapterError> {
    current
        .checked_add(amount)
        .ok_or(AdapterError::ArithmeticOverflow)
}

fn principal_after_decrease(current: i128, amount: i128) -> Result<i128, AdapterError> {
    if current < amount {
        return Err(AdapterError::InsufficientPrincipal);
    }
    current
        .checked_sub(amount)
        .ok_or(AdapterError::ArithmeticUnderflow)
}

fn increase_principal(env: &Env, asset: &Address, amount: i128) -> Result<(), AdapterError> {
    let updated = principal_after_increase(principal_of(env, asset), amount)?;
    env.storage()
        .instance()
        .set(&DataKey::Principal(asset.clone()), &updated);
    Ok(())
}

fn decrease_principal(env: &Env, asset: &Address, amount: i128) -> Result<(), AdapterError> {
    let updated = principal_after_decrease(principal_of(env, asset), amount)?;
    let key = DataKey::Principal(asset.clone());
    if updated == 0 {
        env.storage().instance().remove(&key);
    } else {
        env.storage().instance().set(&key, &updated);
    }
    Ok(())
}

fn returned_of(env: &Env, asset: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::Returned(asset.clone()))
        .unwrap_or(0)
}

fn decrease_returned(env: &Env, asset: &Address, amount: i128) -> Result<(), AdapterError> {
    let current = returned_of(env, asset);
    if current < amount {
        return Err(AdapterError::InsufficientRecordedReturn);
    }
    let updated = current
        .checked_sub(amount)
        .ok_or(AdapterError::ArithmeticUnderflow)?;
    let key = DataKey::Returned(asset.clone());
    if updated == 0 {
        env.storage().instance().remove(&key);
    } else {
        env.storage().instance().set(&key, &updated);
    }
    Ok(())
}

fn get_admin(env: &Env) -> Result<Address, AdapterError> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(AdapterError::MissingConfig)
}

fn get_vault(env: &Env) -> Result<Address, AdapterError> {
    env.storage()
        .instance()
        .get(&DataKey::Vault)
        .ok_or(AdapterError::MissingConfig)
}

fn get_hot_locker(env: &Env) -> Result<Address, AdapterError> {
    env.storage()
        .instance()
        .get(&DataKey::HotLocker)
        .ok_or(AdapterError::MissingConfig)
}

fn get_hot_receiver_id(env: &Env) -> Result<BytesN<32>, AdapterError> {
    env.storage()
        .instance()
        .get(&DataKey::HotReceiverId)
        .ok_or(AdapterError::MissingConfig)
}

fn get_configured_asset(env: &Env) -> Result<Address, AdapterError> {
    env.storage()
        .instance()
        .get(&DataKey::Asset)
        .ok_or(AdapterError::MissingConfig)
}

fn require_configured_asset(env: &Env, asset: &Address) -> Result<(), AdapterError> {
    if asset == &get_configured_asset(env)? {
        Ok(())
    } else {
        Err(AdapterError::UnsupportedAsset)
    }
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    if caller != &get_admin(env)? {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn require_vault(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    if caller != &get_vault(env)? {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn require_hot_locker(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    if caller != &get_hot_locker(env)? {
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

fn is_paused(env: &Env) -> bool {
    matches!(
        env.storage()
            .instance()
            .get(&DataKey::OperationalState)
            .unwrap_or(OperationalState::Active),
        OperationalState::Paused(_, _)
    )
}

fn require_not_paused(env: &Env) -> Result<(), AdapterError> {
    if is_paused(env) {
        return Err(AdapterError::Paused);
    }
    Ok(())
}

fn require_contract_address(addr: &Address, err: AdapterError) -> Result<(), AdapterError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(err)
    }
}

fn is_contract_address(addr: &Address) -> bool {
    matches!(
        AddressPayload::from_address(addr),
        Some(AddressPayload::ContractIdHash(_))
    )
}

impl Upgradeable for HotBridgeAdapterContract {
    #[allow(deprecated)]
    fn upgrade(e: &Env, new_wasm_hash: BytesN<32>, operator: Address) {
        extend_instance_ttl(e);
        require_admin(e, &operator).unwrap_or_else(|err| panic_with_error!(e, err));
        upgradeable::upgrade(e, &new_wasm_hash);
        e.events()
            .publish((symbol_short!("upgrade"), operator), new_wasm_hash);
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

#[allow(unexpected_cfgs)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn choose_hot_client_timestamp_matches_nonce_policy() {
        let base_timestamp: u128 = kani::any();
        let has_last: bool = kani::any();
        let raw_last_timestamp: u128 = kani::any();
        let last_timestamp = has_last.then_some(raw_last_timestamp);

        let selected = choose_hot_client_timestamp(base_timestamp, last_timestamp);

        match last_timestamp {
            None => {
                assert_eq!(selected, Ok(base_timestamp));
            }
            Some(last) if base_timestamp > last => {
                assert_eq!(selected, Ok(base_timestamp));
            }
            Some(0) => {
                assert_eq!(selected, Err(AdapterError::HotClientTimestampExhausted));
            }
            Some(last) => {
                let selected = selected.expect("last > 0 can step down");
                assert_eq!(selected, last - 1);
                assert!(selected < last);
                assert_ne!(selected, last);
            }
        }
    }

    #[kani::proof]
    fn hot_locker_timestamp_handles_boundary_ledger_timestamps() {
        let zero = hot_locker_timestamp(0).expect("zero timestamp scales");
        assert_eq!(zero, 0);

        let one = hot_locker_timestamp(1).expect("one timestamp scales");
        assert_eq!(one, HOT_TIMESTAMP_SCALE);

        let max = hot_locker_timestamp(u64::MAX).expect("max u64 timestamp scales");
        assert_eq!(max, u128::from(u64::MAX) * HOT_TIMESTAMP_SCALE);

        let next_after_max = choose_hot_client_timestamp(max, Some(max))
            .expect("max scaled timestamp can step down");
        assert_eq!(next_after_max, max - 1);
    }

    #[kani::proof]
    fn positive_i128_amounts_convert_losslessly_to_u128() {
        let amount: i128 = kani::any();

        if amount > 0 {
            assert_eq!(require_positive_amount(amount), Ok(()));
            assert_eq!(
                amount_as_u128(amount),
                Ok(u128::try_from(amount).expect("positive i128 always fits u128"))
            );
        } else {
            assert_eq!(
                require_positive_amount(amount),
                Err(AdapterError::InvalidInput)
            );
        }
    }

    #[kani::proof]
    fn principal_increase_is_checked_and_monotonic_for_valid_state() {
        let current: i128 = kani::any();
        let amount: i128 = kani::any();
        kani::assume(current >= 0);
        kani::assume(amount > 0);

        let updated = principal_after_increase(current, amount);

        match current.checked_add(amount) {
            Some(expected) => {
                assert_eq!(updated, Ok(expected));
                let updated = updated.expect("checked add succeeded");
                assert!(updated >= current);
                assert!(updated >= amount);
            }
            None => {
                assert_eq!(updated, Err(AdapterError::ArithmeticOverflow));
            }
        }
    }

    #[kani::proof]
    fn principal_decrease_is_checked_and_never_negative_for_valid_state() {
        let current: i128 = kani::any();
        let amount: i128 = kani::any();
        kani::assume(current >= 0);
        kani::assume(amount > 0);

        let updated = principal_after_decrease(current, amount);

        if amount > current {
            assert_eq!(updated, Err(AdapterError::InsufficientPrincipal));
        } else {
            let updated = updated.expect("sufficient principal should decrease");
            assert_eq!(updated, current - amount);
            assert!(updated >= 0);
            assert!(updated < current);
        }
    }

    #[kani::proof]
    fn supply_balance_postcondition_accepts_only_exact_spend() {
        let balance_before: i128 = kani::any();
        let balance_after: i128 = kani::any();
        let amount: i128 = kani::any();
        kani::assume(balance_before >= 0);
        kani::assume(balance_after >= 0);
        kani::assume(amount > 0);

        let result = validate_supply_balance_change(balance_before, balance_after, amount);

        if balance_before < amount {
            assert_eq!(result, Err(AdapterError::InsufficientAdapterBalance));
        } else if balance_after > balance_before {
            assert_eq!(result, Err(AdapterError::DepositPostconditionFailed));
        } else {
            let spent = balance_before - balance_after;
            if spent == amount {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(result, Err(AdapterError::DepositPostconditionFailed));
            }
        }
    }

    #[kani::proof]
    fn withdrawal_resources_require_principal_and_returned_balance() {
        let principal: i128 = kani::any();
        let adapter_balance: i128 = kani::any();
        let amount: i128 = kani::any();
        kani::assume(principal >= 0);
        kani::assume(adapter_balance >= 0);
        kani::assume(amount > 0);

        let recorded_returned: i128 = kani::any();
        kani::assume(recorded_returned >= 0);

        let result =
            validate_withdrawal_resources(principal, recorded_returned, adapter_balance, amount);

        if principal < amount {
            assert_eq!(result, Err(AdapterError::InsufficientPrincipal));
        } else if recorded_returned < amount {
            assert_eq!(result, Err(AdapterError::InsufficientRecordedReturn));
        } else if adapter_balance < amount {
            assert_eq!(result, Err(AdapterError::InsufficientReturnedBalance));
        } else {
            assert_eq!(result, Ok(()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::{
        contract, contractimpl,
        testutils::{Address as _, EnvTestConfig, Events as _, Ledger as _},
        token::StellarAssetClient,
        xdr::{ContractEventBody, ScVal},
        TryFromVal,
    };

    #[contract]
    struct DummyContract;

    #[contractimpl]
    impl DummyContract {}

    #[contract]
    struct MockHotLocker;

    #[contractimpl]
    impl MockHotLocker {
        pub fn deposit(
            env: Env,
            sender_id: Address,
            amount: u128,
            token: Address,
            receiver_id: BytesN<32>,
            client_timestamp: u128,
        ) -> u128 {
            let amount_i128 = i128::try_from(amount).unwrap();
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "sender"), &sender_id);
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "receiver"), &receiver_id);
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "token"), &token);
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "timestamp"), &client_timestamp);
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "amount"), &amount_i128);
            soroban_sdk::token::Client::new(&env, &token).transfer(
                &sender_id,
                env.current_contract_address(),
                &amount_i128,
            );
            client_timestamp
        }

        pub fn amount(env: Env) -> i128 {
            env.storage()
                .instance()
                .get(&Symbol::new(&env, "amount"))
                .unwrap()
        }

        pub fn timestamp(env: Env) -> u128 {
            env.storage()
                .instance()
                .get(&Symbol::new(&env, "timestamp"))
                .unwrap()
        }

        pub fn receiver(env: Env) -> BytesN<32> {
            env.storage()
                .instance()
                .get(&Symbol::new(&env, "receiver"))
                .unwrap()
        }
    }

    #[contract]
    struct MockNoTransferHotLocker;

    #[contractimpl]
    impl MockNoTransferHotLocker {
        pub fn deposit(
            _env: Env,
            _sender_id: Address,
            _amount: u128,
            _token: Address,
            _receiver_id: BytesN<32>,
            client_timestamp: u128,
        ) -> u128 {
            client_timestamp
        }
    }

    fn register_dummy_contract(env: &Env) -> Address {
        env.register(DummyContract, ())
    }

    fn setup(env: &Env) -> (Address, Address, Address, Address, Address, BytesN<32>) {
        env.mock_all_auths();
        env.ledger().set_timestamp(1_777_000_000);
        let admin = register_dummy_contract(env);
        let vault = register_dummy_contract(env);
        let hot_locker = env.register(MockHotLocker, ());
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(env));
        let asset = asset_sac.address();
        let receiver = BytesN::from_array(
            env,
            &[
                0x52, 0xfd, 0x58, 0x1d, 0xe4, 0x1f, 0x4b, 0xac, 0xe8, 0x8c, 0x93, 0x6b, 0x89, 0xbf,
                0x26, 0x7a, 0x11, 0x61, 0x42, 0x6a, 0x46, 0x6a, 0xdc, 0x51, 0x8c, 0xd9, 0xe5, 0x6f,
                0x20, 0x16, 0x51, 0xdd,
            ],
        );
        let adapter = env.register(
            HotBridgeAdapterContract,
            (&admin, &vault, &hot_locker, &asset, &receiver),
        );
        (adapter, admin, vault, hot_locker, asset, receiver)
    }

    fn expected_hot_client_timestamp(env: &Env, same_ledger_sequence: u128) -> u128 {
        u128::from(env.ledger().timestamp()) * 1_000_000_000_000 - same_ledger_sequence
    }

    fn fuzz_env() -> Env {
        Env::new_with_config(EnvTestConfig {
            capture_snapshot_at_drop: false,
        })
    }

    fn assert_hot_deposit_event(
        env: &Env,
        adapter: &Address,
        asset: &Address,
        hot_locker: &Address,
        receiver: &BytesN<32>,
        amount: i128,
        nonce: u128,
    ) {
        let hot_deposit = ScVal::try_from_val(env, &symbol_short!("hot_dep")).unwrap();
        let asset_val: Val = asset.clone().into_val(env);
        let asset_scval = ScVal::try_from_val(env, &asset_val).unwrap();
        let data_val: Val = (amount, hot_locker.clone(), receiver.clone(), nonce).into_val(env);
        let data_scval = ScVal::try_from_val(env, &data_val).unwrap();
        let filtered_events = env.events().all().filter_by_contract(adapter);
        let events = filtered_events.events();

        assert!(events.iter().any(|event| {
            let ContractEventBody::V0(body) = &event.body;
            body.topics.len() == 2
                && body.topics[0] == hot_deposit
                && body.topics[1] == asset_scval
                && body.data == data_scval
        }));
    }

    fn choose_expected_timestamp(base_timestamp: u128, last_timestamp: &mut Option<u128>) -> u128 {
        let selected = choose_hot_client_timestamp(base_timestamp, *last_timestamp)
            .expect("test timestamps should not exhaust");
        *last_timestamp = Some(selected);
        selected
    }

    #[derive(Clone, Debug)]
    enum AdapterOp {
        Supply { amount: i128, ledger_delta: u64 },
        Withdraw { amount: i128 },
    }

    fn adapter_op_strategy() -> impl Strategy<Value = AdapterOp> {
        prop_oneof![
            (1i128..=50_000i128, 0u64..=2u64).prop_map(|(amount, ledger_delta)| {
                AdapterOp::Supply {
                    amount,
                    ledger_delta,
                }
            }),
            (1i128..=50_000i128).prop_map(|amount| AdapterOp::Withdraw { amount }),
        ]
    }

    proptest! {
        #[test]
        fn prop_hot_timestamp_selection_never_reuses_last(
            base_timestamp in any::<u128>(),
            last_timestamp in prop::option::of(any::<u128>()),
        ) {
            let selected = choose_hot_client_timestamp(base_timestamp, last_timestamp);

            match last_timestamp {
                None => {
                    prop_assert_eq!(selected, Ok(base_timestamp));
                }
                Some(last) if base_timestamp > last => {
                    prop_assert_eq!(selected, Ok(base_timestamp));
                }
                Some(0) => {
                    prop_assert_eq!(selected, Err(AdapterError::HotClientTimestampExhausted));
                }
                Some(last) => {
                    let selected = selected.expect("last > 0 should step down");
                    prop_assert_eq!(selected, last - 1);
                    prop_assert!(selected < last);
                }
            }
        }

        #[test]
        fn prop_supply_and_withdraw_sequences_preserve_principal_and_balances(
            ops in prop::collection::vec(adapter_op_strategy(), 1..25),
        ) {
            let env = fuzz_env();
            let (adapter, _admin, vault, hot_locker, asset, _receiver) = setup(&env);
            let client = HotBridgeAdapterContractClient::new(&env, &adapter);
            let asset_admin = StellarAssetClient::new(&env, &asset);
            let token = soroban_sdk::token::Client::new(&env, &asset);
            let locker_client = MockHotLockerClient::new(&env, &hot_locker);
            let mut model_principal = 0i128;
            let mut model_vault_balance = 0i128;
            let mut model_locker_balance = 0i128;
            let mut ledger_timestamp = 1_777_000_000u64;
            let mut last_hot_timestamp = None;

            for op in ops {
                match op {
                    AdapterOp::Supply { amount, ledger_delta } => {
                        ledger_timestamp = ledger_timestamp.saturating_add(ledger_delta);
                        env.ledger().set_timestamp(ledger_timestamp);
                        let expected_nonce = choose_expected_timestamp(
                            hot_locker_timestamp(ledger_timestamp).expect("bounded ledger timestamp"),
                            &mut last_hot_timestamp,
                        );

                        asset_admin.mint(&vault, &amount);
                        client.supply(&vault, &asset, &amount);
                        model_principal += amount;
                        model_locker_balance += amount;

                        prop_assert_eq!(locker_client.timestamp(), expected_nonce);
                        prop_assert_eq!(locker_client.amount(), amount);
                    }
                    AdapterOp::Withdraw { amount } => {
                        let result = client.try_progress_withdrawal(&vault, &asset, &amount);
                        if amount > model_principal {
                            prop_assert_eq!(result, Err(Ok(AdapterError::InsufficientPrincipal)));
                        } else {
                            prop_assert_eq!(result, Err(Ok(AdapterError::InsufficientRecordedReturn)));

                            asset_admin.mint(&adapter, &amount);
                            client.record_returned(&hot_locker, &asset, &amount);
                            prop_assert_eq!(client.progress_withdrawal(&vault, &asset, &amount), amount);
                            model_principal -= amount;
                            model_vault_balance += amount;
                        }
                    }
                }

                prop_assert_eq!(client.total_assets(&asset), model_principal);
                prop_assert_eq!(token.balance(&vault), model_vault_balance);
                prop_assert_eq!(token.balance(&hot_locker), model_locker_balance);
                prop_assert_eq!(token.balance(&adapter), 0);
            }
        }

        #[test]
        fn prop_failed_no_transfer_supply_preserves_principal_and_balance(amount in 1i128..=1_000_000i128) {
            let env = fuzz_env();
            env.mock_all_auths();
            let admin = register_dummy_contract(&env);
            let vault = register_dummy_contract(&env);
            let hot_locker = env.register(MockNoTransferHotLocker, ());
            let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
            let asset = asset_sac.address();
            let receiver = BytesN::from_array(&env, &[7; 32]);
            let adapter = env.register(
                HotBridgeAdapterContract,
                (&admin, &vault, &hot_locker, &asset, &receiver),
            );
            let client = HotBridgeAdapterContractClient::new(&env, &adapter);
            let token = soroban_sdk::token::Client::new(&env, &asset);
            StellarAssetClient::new(&env, &asset).mint(&vault, &amount);

            prop_assert_eq!(client.try_supply(&vault, &asset, &amount), Err(Ok(AdapterError::DepositPostconditionFailed)));
            prop_assert_eq!(client.total_assets(&asset), 0);
            prop_assert_eq!(token.balance(&adapter), 0);
            prop_assert_eq!(token.balance(&vault), amount);
            prop_assert_eq!(token.balance(&hot_locker), 0);
        }
    }

    #[test]
    fn supply_calls_hot_locker_with_configured_receiver_id() {
        let env = Env::default();
        let (adapter, _admin, vault, hot_locker, asset, receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);

        client.supply(&vault, &asset, &100);
        assert_hot_deposit_event(
            &env,
            &adapter,
            &asset,
            &hot_locker,
            &receiver,
            100,
            expected_hot_client_timestamp(&env, 0),
        );

        let locker_client = MockHotLockerClient::new(&env, &hot_locker);
        assert_eq!(locker_client.receiver(), receiver);
        assert_eq!(locker_client.amount(), 100);
        assert_eq!(
            locker_client.timestamp(),
            expected_hot_client_timestamp(&env, 0)
        );
        assert_eq!(client.total_assets(&asset), 100);
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&adapter),
            0
        );
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&hot_locker),
            100
        );
    }

    #[test]
    fn supply_uses_unique_hot_nonce_for_same_ledger_allocations() {
        let env = Env::default();
        let (adapter, _admin, vault, hot_locker, asset, receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        let base_nonce = expected_hot_client_timestamp(&env, 0);

        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);
        assert_hot_deposit_event(
            &env,
            &adapter,
            &asset,
            &hot_locker,
            &receiver,
            100,
            base_nonce,
        );

        asset_admin.mint(&vault, &60);
        client.supply(&vault, &asset, &60);
        assert_hot_deposit_event(
            &env,
            &adapter,
            &asset,
            &hot_locker,
            &receiver,
            60,
            base_nonce - 1,
        );
        assert_eq!(client.total_assets(&asset), 160);

        env.ledger().set_timestamp(1_777_000_001);
        asset_admin.mint(&vault, &25);
        client.supply(&vault, &asset, &25);

        assert_hot_deposit_event(
            &env,
            &adapter,
            &asset,
            &hot_locker,
            &receiver,
            25,
            expected_hot_client_timestamp(&env, 0),
        );
        assert_eq!(client.total_assets(&asset), 185);
    }

    #[test]
    fn progress_withdrawal_returns_hot_completed_balance_to_vault() {
        let env = Env::default();
        let (adapter, _admin, vault, _hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);

        asset_admin.mint(&adapter, &40);
        client.record_returned(&_hot_locker, &asset, &40);

        assert_eq!(client.progress_withdrawal(&vault, &asset, &40), 40);
        assert_eq!(client.total_assets(&asset), 60);
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&vault),
            40
        );
    }

    #[test]
    fn progress_withdrawal_requires_returned_balance() {
        let env = Env::default();
        let (adapter, _admin, vault, _hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);

        let error = client.try_progress_withdrawal(&vault, &asset, &1);
        assert_eq!(error, Err(Ok(AdapterError::InsufficientRecordedReturn)));
    }

    #[test]
    fn supply_rejects_redeploying_returned_balance() {
        let env = Env::default();
        let (adapter, _admin, vault, hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);

        asset_admin.mint(&adapter, &40);
        client.record_returned(&hot_locker, &asset, &40);
        asset_admin.mint(&vault, &40);

        let error = client.try_supply(&vault, &asset, &40);

        assert_eq!(error, Err(Ok(AdapterError::PendingReturnedBalance)));
        assert_eq!(client.total_assets(&asset), 100);
        assert_eq!(client.returned_balance(&asset), 40);
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&adapter),
            40
        );
    }

    #[test]
    fn supply_rejects_unsolicited_adapter_balance() {
        let env = Env::default();
        let (adapter, _admin, vault, _hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);

        asset_admin.mint(&adapter, &40);
        asset_admin.mint(&vault, &40);

        let error = client.try_supply(&vault, &asset, &40);

        assert_eq!(error, Err(Ok(AdapterError::InsufficientAdapterBalance)));
        assert_eq!(client.total_assets(&asset), 100);
    }

    #[test]
    fn progress_withdrawal_rejects_unsolicited_balance_without_return_record() {
        let env = Env::default();
        let (adapter, _admin, vault, _hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);

        asset_admin.mint(&adapter, &50);

        let error = client.try_progress_withdrawal(&vault, &asset, &50);

        assert_eq!(error, Err(Ok(AdapterError::InsufficientRecordedReturn)));
        assert_eq!(client.total_assets(&asset), 100);
    }

    #[test]
    fn rescue_cannot_transfer_principal_backing_returned_balance() {
        let env = Env::default();
        let (adapter, _admin, vault, hot_locker, asset, _receiver) = setup(&env);
        let receiver = register_dummy_contract(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&vault, &100);
        client.supply(&vault, &asset, &100);
        asset_admin.mint(&adapter, &40);
        client.record_returned(&hot_locker, &asset, &40);

        let error = client.try_rescue(&vault, &asset, &40, &receiver);

        assert_eq!(error, Err(Ok(AdapterError::InsufficientSurplus)));
        assert_eq!(client.total_assets(&asset), 100);
        assert_eq!(client.returned_balance(&asset), 40);
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&adapter),
            40
        );
    }

    #[test]
    fn supply_rejects_unsupported_asset() {
        let env = Env::default();
        let (adapter, _admin, vault, _hot_locker, _configured_asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let unsupported_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let unsupported_asset = unsupported_sac.address();
        StellarAssetClient::new(&env, &unsupported_asset).mint(&vault, &1);

        let error = client.try_supply(&vault, &unsupported_asset, &1);

        assert_eq!(error, Err(Ok(AdapterError::UnsupportedAsset)));
    }

    #[test]
    fn supply_rejects_locker_that_does_not_pull_exact_amount() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = register_dummy_contract(&env);
        let vault = register_dummy_contract(&env);
        let hot_locker = env.register(MockNoTransferHotLocker, ());
        let asset_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
        let asset = asset_sac.address();
        let receiver = BytesN::from_array(&env, &[7; 32]);
        let adapter = env.register(
            HotBridgeAdapterContract,
            (&admin, &vault, &hot_locker, &asset, &receiver),
        );
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        StellarAssetClient::new(&env, &asset).mint(&vault, &100);

        let error = client.try_supply(&vault, &asset, &100);

        assert_eq!(error, Err(Ok(AdapterError::DepositPostconditionFailed)));
        assert_eq!(client.total_assets(&asset), 0);
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&adapter),
            0
        );
        assert_eq!(
            soroban_sdk::token::Client::new(&env, &asset).balance(&vault),
            100
        );
    }

    #[test]
    fn non_vault_cannot_supply() {
        let env = Env::default();
        let (adapter, _admin, _vault, _hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let caller = register_dummy_contract(&env);

        let error = client.try_supply(&caller, &asset, &1);
        assert_eq!(error, Err(Ok(AdapterError::Unauthorized)));
    }
}
