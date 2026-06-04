use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, Events as _, Ledger as _},
    token::StellarAssetClient,
    xdr::{ContractEventBody, ScVal},
    Address, Bytes, BytesN, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};
use templar_soroban_hot_bridge_adapter::{
    HotBridgeAdapterContract, HotBridgeAdapterContractClient,
};

const HOT_LOCKER_WASM: &[u8] = include_bytes!(
    "fixtures/hot_stellar_locker_mainnet_cc74acb56fd01ef560b5d443cd58fe11d5acbece0e4001612ebb31020fc92f97.wasm"
);
const HOT_LOCKER_SHA256: [u8; 32] = [
    0xcc, 0x74, 0xac, 0xb5, 0x6f, 0xd0, 0x1e, 0xf5, 0x60, 0xb5, 0xd4, 0x43, 0xcd, 0x58, 0xfe, 0x11,
    0xd5, 0xac, 0xbe, 0xce, 0x0e, 0x40, 0x01, 0x61, 0x2e, 0xbb, 0x31, 0x02, 0x0f, 0xc9, 0x2f, 0x97,
];
const HOT_PUBLIC_KEY: [u8; 65] = [
    0x04, 0x4d, 0x4a, 0xad, 0x55, 0xb2, 0x48, 0x97, 0x2a, 0x44, 0xbc, 0x5d, 0x78, 0x1e, 0x2e, 0xa9,
    0x86, 0x46, 0x9d, 0xac, 0xbe, 0xb2, 0xf3, 0x43, 0xf3, 0xa9, 0xaa, 0xe7, 0x27, 0x0c, 0x75, 0x42,
    0xcb, 0xf7, 0xb0, 0x2a, 0x79, 0x49, 0xaf, 0x99, 0x8d, 0xe3, 0xaa, 0xb8, 0x90, 0xd5, 0xc9, 0xea,
    0xe2, 0x9e, 0x5e, 0xe5, 0x27, 0x73, 0x65, 0xf5, 0xaf, 0xe4, 0x9b, 0x40, 0x68, 0x25, 0x61, 0xf3,
    0xaf,
];
const PROVEN_HOT_RECEIVER_ID: [u8; 32] = [
    0x52, 0xfd, 0x58, 0x1d, 0xe4, 0x1f, 0x4b, 0xac, 0xe8, 0x8c, 0x93, 0x6b, 0x89, 0xbf, 0x26, 0x7a,
    0x11, 0x61, 0x42, 0x6a, 0x46, 0x6a, 0xdc, 0x51, 0x8c, 0xd9, 0xe5, 0x6f, 0x20, 0x16, 0x51, 0xdd,
];

#[contract]
struct DummyContract;

#[contractimpl]
impl DummyContract {}

fn dummy_contract(env: &Env) -> Address {
    env.register(DummyContract, ())
}

fn hot_public_key(env: &Env) -> BytesN<65> {
    BytesN::from_array(env, &HOT_PUBLIC_KEY)
}

fn proven_receiver(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &PROVEN_HOT_RECEIVER_ID)
}

fn register_mainnet_locker_wasm(env: &Env) -> Address {
    let locker = env.register(HOT_LOCKER_WASM, ());
    let args: Vec<Val> = (hot_public_key(env), dummy_contract(env)).into_val(env);
    env.invoke_contract::<()>(&locker, &Symbol::new(env, "constructor"), args);
    locker
}

fn invoke_locker_deposit(
    env: &Env,
    locker: &Address,
    sender: &Address,
    amount: u128,
    token: &Address,
    receiver: &BytesN<32>,
    client_timestamp: u128,
) -> u128 {
    let args: Vec<Val> = (
        sender.clone(),
        amount,
        token.clone(),
        receiver.clone(),
        client_timestamp,
    )
        .into_val(env);
    env.invoke_contract(locker, &Symbol::new(env, "deposit"), args)
}

fn get_locker_deposit(env: &Env, locker: &Address, nonce: u128) -> Bytes {
    let args: Vec<Val> = (nonce,).into_val(env);
    env.invoke_contract(locker, &Symbol::new(env, "get_deposit"), args)
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
fn pinned_fixture_matches_mainnet_locker_hash() {
    let env = Env::default();
    let bytes = Bytes::from_slice(&env, HOT_LOCKER_WASM);
    let digest = env.crypto().sha256(&bytes).to_bytes().to_array();
    assert_eq!(digest, HOT_LOCKER_SHA256);
}

#[test]
fn mainnet_locker_wasm_accepts_verified_deposit_shape() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_777_000_000);

    let locker = register_mainnet_locker_wasm(&env);
    let token_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token = token_sac.address();
    let token_admin = StellarAssetClient::new(&env, &token);
    let sender = dummy_contract(&env);
    let receiver = proven_receiver(&env);
    token_admin.mint(&sender, &100);

    let nonce = invoke_locker_deposit(
        &env,
        &locker,
        &sender,
        100,
        &token,
        &receiver,
        1_777_000_000_000_000_000_000,
    );

    assert_eq!(
        soroban_sdk::token::Client::new(&env, &token).balance(&sender),
        0
    );
    assert_eq!(
        soroban_sdk::token::Client::new(&env, &token).balance(&locker),
        100
    );
    assert!(!get_locker_deposit(&env, &locker, nonce).is_empty());
}

#[test]
#[should_panic(expected = "HostError: Error(WasmVm, InvalidAction)")]
fn mainnet_locker_wasm_rejects_duplicate_client_timestamp_in_same_ledger() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_777_000_000);

    let locker = register_mainnet_locker_wasm(&env);
    let token_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token = token_sac.address();
    let token_admin = StellarAssetClient::new(&env, &token);
    let first_sender = dummy_contract(&env);
    let second_sender = dummy_contract(&env);
    let receiver = proven_receiver(&env);
    let client_timestamp = 1_777_000_000_000_000_000_000;
    token_admin.mint(&first_sender, &100);
    token_admin.mint(&second_sender, &60);

    let first_nonce = invoke_locker_deposit(
        &env,
        &locker,
        &first_sender,
        100,
        &token,
        &receiver,
        client_timestamp,
    );
    assert_eq!(first_nonce, client_timestamp);
    assert!(!get_locker_deposit(&env, &locker, first_nonce).is_empty());

    let _second_nonce = invoke_locker_deposit(
        &env,
        &locker,
        &second_sender,
        60,
        &token,
        &receiver,
        client_timestamp,
    );
}

#[test]
fn adapter_supply_works_against_mainnet_locker_wasm() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_777_000_000);

    let admin = dummy_contract(&env);
    let vault = dummy_contract(&env);
    let locker = register_mainnet_locker_wasm(&env);
    let token_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token = token_sac.address();
    let receiver = proven_receiver(&env);
    let adapter = env.register(
        HotBridgeAdapterContract,
        (&admin, &vault, &locker, &receiver),
    );
    let adapter_client = HotBridgeAdapterContractClient::new(&env, &adapter);
    StellarAssetClient::new(&env, &token).mint(&adapter, &100);

    adapter_client.supply(&vault, &token, &100);
    assert_hot_deposit_event(
        &env,
        &adapter,
        &token,
        &locker,
        &receiver,
        100,
        1_777_000_000_000_000_000_000,
    );

    assert_eq!(adapter_client.total_assets(&token), 100);
    assert_eq!(
        soroban_sdk::token::Client::new(&env, &token).balance(&adapter),
        0
    );
    assert_eq!(
        soroban_sdk::token::Client::new(&env, &token).balance(&locker),
        100
    );
}

#[test]
fn adapter_supply_supports_two_deposits_in_same_ledger_against_mainnet_locker_wasm() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_777_000_000);

    let admin = dummy_contract(&env);
    let vault = dummy_contract(&env);
    let locker = register_mainnet_locker_wasm(&env);
    let token_sac = env.register_stellar_asset_contract_v2(Address::generate(&env));
    let token = token_sac.address();
    let receiver = proven_receiver(&env);
    let adapter = env.register(
        HotBridgeAdapterContract,
        (&admin, &vault, &locker, &receiver),
    );
    let adapter_client = HotBridgeAdapterContractClient::new(&env, &adapter);
    StellarAssetClient::new(&env, &token).mint(&adapter, &100);
    adapter_client.supply(&vault, &token, &100);

    let first_nonce = 1_777_000_000_000_000_000_000;
    let second_nonce = first_nonce - 1;
    assert_hot_deposit_event(&env, &adapter, &token, &locker, &receiver, 100, first_nonce);

    StellarAssetClient::new(&env, &token).mint(&adapter, &60);
    adapter_client.supply(&vault, &token, &60);
    assert_hot_deposit_event(&env, &adapter, &token, &locker, &receiver, 60, second_nonce);
    assert!(!get_locker_deposit(&env, &locker, first_nonce).is_empty());
    assert!(!get_locker_deposit(&env, &locker, second_nonce).is_empty());
    assert_eq!(adapter_client.total_assets(&token), 160);
    assert_eq!(
        soroban_sdk::token::Client::new(&env, &token).balance(&locker),
        160
    );
}
