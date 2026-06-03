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

#[contracttype]
#[derive(Clone, Debug)]
enum DataKey {
    Admin,
    PendingAdmin,
    Vault,
    HotLocker,
    HotReceiverId,
    Paused,
    Principal(Address),
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
}

#[contract]
pub struct HotBridgeAdapterContract;

#[contractimpl]
impl HotBridgeAdapterContract {
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        hot_locker: Address,
        hot_receiver_id: BytesN<32>,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_contract_address(&admin, AdapterError::InvalidInput)?;
        require_contract_address(&vault, AdapterError::InvalidInput)?;
        require_contract_address(&hot_locker, AdapterError::InvalidInput)?;

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage()
            .instance()
            .set(&DataKey::HotLocker, &hot_locker);
        env.storage()
            .instance()
            .set(&DataKey::HotReceiverId, &hot_receiver_id);
        Ok(())
    }

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

        let adapter = env.current_contract_address();
        let hot_locker = get_hot_locker(&env)?;
        let hot_receiver_id = get_hot_receiver_id(&env)?;
        let client_timestamp = hot_locker_timestamp(&env)?;
        let amount_u128 = amount_as_u128(amount)?;
        let token = soroban_sdk::token::Client::new(&env, &asset);
        let balance_before = token.balance(&adapter);
        if balance_before < amount {
            return Err(AdapterError::InsufficientAdapterBalance);
        }

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
        let spent = balance_before
            .checked_sub(balance_after)
            .ok_or(AdapterError::DepositPostconditionFailed)?;
        if spent != amount {
            return Err(AdapterError::DepositPostconditionFailed);
        }

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

        let principal = principal_of(&env, &asset);
        if principal < amount {
            return Err(AdapterError::InsufficientPrincipal);
        }

        let adapter = env.current_contract_address();
        let vault = get_vault(&env)?;
        let token = soroban_sdk::token::Client::new(&env, &asset);
        if token.balance(&adapter) < amount {
            return Err(AdapterError::InsufficientReturnedBalance);
        }

        token.transfer(&adapter, &vault, &amount);
        decrease_principal(&env, &asset, amount)?;
        env.events()
            .publish((symbol_short!("withdraw"), asset), amount);
        Ok(amount)
    }

    pub fn total_assets(env: Env, asset: Address) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        Ok(principal_of(&env, &asset))
    }

    pub fn principal(env: Env, asset: Address) -> i128 {
        extend_instance_ttl(&env);
        principal_of(&env, &asset)
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

    pub fn propose_admin(
        env: Env,
        caller: Address,
        new_admin: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        require_contract_address(&new_admin, AdapterError::InvalidInput)?;
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
        env.events()
            .publish((symbol_short!("adm_prop"), caller), new_admin);
        Ok(())
    }

    pub fn accept_admin(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        caller.require_auth();
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(AdapterError::MissingConfig)?;
        if caller != pending {
            return Err(AdapterError::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Admin, &caller);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        env.events().publish((symbol_short!("adm_acc"), caller), ());
        Ok(())
    }

    pub fn pending_admin(env: Env) -> Option<Address> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&DataKey::PendingAdmin)
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
        require_contract_address(&receiver, AdapterError::InvalidInput)?;
        if receiver == env.current_contract_address() {
            return Err(AdapterError::InvalidInput);
        }

        let adapter = env.current_contract_address();
        soroban_sdk::token::Client::new(&env, &asset).transfer(&adapter, &receiver, &amount);
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

fn hot_locker_timestamp(env: &Env) -> Result<u128, AdapterError> {
    u128::from(env.ledger().timestamp())
        .checked_mul(1_000_000_000_000)
        .ok_or(AdapterError::ArithmeticOverflow)
}

fn principal_of(env: &Env, asset: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::Principal(asset.clone()))
        .unwrap_or(0)
}

fn increase_principal(env: &Env, asset: &Address, amount: i128) -> Result<(), AdapterError> {
    let updated = principal_of(env, asset)
        .checked_add(amount)
        .ok_or(AdapterError::ArithmeticOverflow)?;
    env.storage()
        .instance()
        .set(&DataKey::Principal(asset.clone()), &updated);
    Ok(())
}

fn decrease_principal(env: &Env, asset: &Address, amount: i128) -> Result<(), AdapterError> {
    let principal = principal_of(env, asset);
    if principal < amount {
        return Err(AdapterError::InsufficientPrincipal);
    }
    let updated = principal
        .checked_sub(amount)
        .ok_or(AdapterError::ArithmeticUnderflow)?;
    let key = DataKey::Principal(asset.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contractimpl,
        testutils::{Address as _, Events as _, Ledger as _},
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
                &env.current_contract_address(),
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
            (&admin, &vault, &hot_locker, &receiver),
        );
        (adapter, admin, vault, hot_locker, asset, receiver)
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

    #[test]
    fn supply_calls_hot_locker_with_configured_receiver_id() {
        let env = Env::default();
        let (adapter, _admin, vault, hot_locker, asset, receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&adapter, &100);

        client.supply(&vault, &asset, &100);
        assert_hot_deposit_event(
            &env,
            &adapter,
            &asset,
            &hot_locker,
            &receiver,
            100,
            1_777_000_000_000_000_000_000,
        );

        let locker_client = MockHotLockerClient::new(&env, &hot_locker);
        assert_eq!(locker_client.receiver(), receiver);
        assert_eq!(locker_client.amount(), 100);
        assert_eq!(locker_client.timestamp(), 1_777_000_000_000_000_000_000);
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
    fn progress_withdrawal_returns_hot_completed_balance_to_vault() {
        let env = Env::default();
        let (adapter, _admin, vault, _hot_locker, asset, _receiver) = setup(&env);
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        let asset_admin = StellarAssetClient::new(&env, &asset);
        asset_admin.mint(&adapter, &100);
        client.supply(&vault, &asset, &100);

        asset_admin.mint(&adapter, &40);

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
        asset_admin.mint(&adapter, &100);
        client.supply(&vault, &asset, &100);

        let error = client.try_progress_withdrawal(&vault, &asset, &1);
        assert_eq!(error, Err(Ok(AdapterError::InsufficientReturnedBalance)));
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
            (&admin, &vault, &hot_locker, &receiver),
        );
        let client = HotBridgeAdapterContractClient::new(&env, &adapter);
        StellarAssetClient::new(&env, &asset).mint(&adapter, &100);

        let error = client.try_supply(&vault, &asset, &100);

        assert_eq!(error, Err(Ok(AdapterError::DepositPostconditionFailed)));
        assert_eq!(client.total_assets(&asset), 0);
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
