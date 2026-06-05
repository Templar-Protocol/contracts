use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _, LedgerInfo},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, IntoVal, Symbol,
};
use templar_4626_proxy_soroban::Soroban4626ProxyContract;
use templar_soroban_runtime::SorobanVaultContract;
use templar_soroban_shared_types::{ProxyViewFields, ProxyViewResponse};
use templar_vault_kernel::DEFAULT_COOLDOWN_NS;

const INITIAL_TIMESTAMP: u64 = 100;
const AUTH_EXPIRATION_LEDGER: u32 = 200;

#[contract]
struct DummyGovernanceContract;

#[contractimpl]
impl DummyGovernanceContract {
    pub fn noop() {}
}

#[derive(Clone)]
struct Users {
    curator: Address,
    governance: Address,
    asset_token_admin: Address,
    user: Address,
    receiver: Address,
}

struct Harness {
    env: Env,
    vault: Address,
    proxy: Address,
    asset_token: Address,
    share_token: Address,
    users: Users,
}

fn setup_harness() -> Harness {
    let env = Env::default();
    env.mock_all_auths();
    set_timestamp(&env, INITIAL_TIMESTAMP);

    let users = Users {
        curator: Address::generate(&env),
        governance: env.register(DummyGovernanceContract, ()),
        asset_token_admin: Address::generate(&env),
        user: Address::generate(&env),
        receiver: Address::generate(&env),
    };

    let vault = env.register(SorobanVaultContract, ());
    let proxy = env.register(Soroban4626ProxyContract, ());
    let asset_token = env
        .register_stellar_asset_contract_v2(users.asset_token_admin.clone())
        .address();
    let share_token = env
        .register_stellar_asset_contract_v2(vault.clone())
        .address();

    env.invoke_contract::<()>(
        &vault,
        &Symbol::new(&env, "initialize"),
        (
            &users.curator,
            &users.governance,
            &asset_token,
            &share_token,
            &0i128,
            &0i128,
        )
            .into_val(&env),
    );

    env.invoke_contract::<()>(
        &proxy,
        &Symbol::new(&env, "initialize"),
        (&vault, &asset_token, &share_token).into_val(&env),
    );

    Harness {
        env,
        vault,
        proxy,
        asset_token,
        share_token,
        users,
    }
}

fn set_timestamp(env: &Env, timestamp: u64) {
    env.ledger().set(LedgerInfo {
        timestamp,
        protocol_version: 25,
        sequence_number: 1,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 1_000,
        ..Default::default()
    });
}

fn advance_past_cooldown(env: &Env) {
    let cooldown_seconds = DEFAULT_COOLDOWN_NS / 1_000_000_000;
    set_timestamp(
        env,
        env.ledger()
            .timestamp()
            .saturating_add(cooldown_seconds)
            .saturating_add(1),
    );
}

fn asset_admin_client(harness: &Harness) -> StellarAssetClient<'_> {
    StellarAssetClient::new(&harness.env, &harness.asset_token)
}

fn asset_client(harness: &Harness) -> TokenClient<'_> {
    TokenClient::new(&harness.env, &harness.asset_token)
}

fn share_client(harness: &Harness) -> TokenClient<'_> {
    TokenClient::new(&harness.env, &harness.share_token)
}

fn vault_total_shares(harness: &Harness) -> i128 {
    vault_proxy_fields(harness, &harness.proxy, 0, 0)
        .core
        .totals
        .total_shares
}

fn mint_and_approve_assets(harness: &Harness, owner: &Address, amount: i128) {
    asset_admin_client(harness).mint(owner, &amount);
    asset_client(harness).approve(owner, &harness.proxy, &amount, &AUTH_EXPIRATION_LEDGER);
}

fn proxy_deposit(harness: &Harness, caller: &Address, assets: i128, receiver: &Address) -> i128 {
    harness.env.invoke_contract::<i128>(
        &harness.proxy,
        &Symbol::new(&harness.env, "deposit"),
        (caller, &assets, receiver).into_val(&harness.env),
    )
}

fn proxy_request_withdraw(
    harness: &Harness,
    owner: &Address,
    receiver: &Address,
    shares: i128,
    min_assets_out: i128,
) -> u64 {
    harness.env.invoke_contract::<u64>(
        &harness.proxy,
        &Symbol::new(&harness.env, "request_withdraw"),
        (owner, receiver, &shares, &min_assets_out).into_val(&harness.env),
    )
}

fn proxy_execute_withdraw(harness: &Harness, caller: &Address) {
    harness.env.invoke_contract::<()>(
        &harness.proxy,
        &Symbol::new(&harness.env, "execute_withdraw"),
        (caller,).into_val(&harness.env),
    );
}

fn proxy_total_assets(harness: &Harness) -> i128 {
    harness.env.invoke_contract::<i128>(
        &harness.proxy,
        &Symbol::new(&harness.env, "total_assets"),
        soroban_sdk::vec![&harness.env],
    )
}

fn proxy_convert_to_shares(harness: &Harness, assets: i128) -> i128 {
    harness.env.invoke_contract::<i128>(
        &harness.proxy,
        &Symbol::new(&harness.env, "convert_to_shares"),
        (&assets,).into_val(&harness.env),
    )
}

fn proxy_convert_to_assets(harness: &Harness, shares: i128) -> i128 {
    harness.env.invoke_contract::<i128>(
        &harness.proxy,
        &Symbol::new(&harness.env, "convert_to_assets"),
        (&shares,).into_val(&harness.env),
    )
}

fn proxy_max_deposit(harness: &Harness, receiver: &Address) -> i128 {
    harness.env.invoke_contract::<i128>(
        &harness.proxy,
        &Symbol::new(&harness.env, "max_deposit"),
        (receiver,).into_val(&harness.env),
    )
}

fn proxy_withdraw(
    harness: &Harness,
    caller: &Address,
    assets: i128,
    receiver: &Address,
    owner: &Address,
) -> u64 {
    harness.env.invoke_contract::<u64>(
        &harness.proxy,
        &Symbol::new(&harness.env, "withdraw"),
        (caller, &assets, receiver, owner).into_val(&harness.env),
    )
}

fn proxy_redeem(
    harness: &Harness,
    caller: &Address,
    shares: i128,
    receiver: &Address,
    owner: &Address,
) -> u64 {
    harness.env.invoke_contract::<u64>(
        &harness.proxy,
        &Symbol::new(&harness.env, "redeem"),
        (caller, &shares, receiver, owner).into_val(&harness.env),
    )
}

fn vault_proxy_view(
    harness: &Harness,
    owner: &Address,
    assets: i128,
    shares: i128,
) -> ProxyViewResponse {
    harness.env.invoke_contract::<ProxyViewResponse>(
        &harness.vault,
        &Symbol::new(&harness.env, "proxy_view"),
        (owner, &assets, &shares).into_val(&harness.env),
    )
}

fn vault_proxy_fields(
    harness: &Harness,
    owner: &Address,
    assets: i128,
    shares: i128,
) -> ProxyViewFields {
    vault_proxy_view(harness, owner, assets, shares).into()
}

#[test]
fn deposit_flow_mints_shares_and_increases_total_assets() {
    let harness = setup_harness();
    let deposit_assets = 500_i128;

    mint_and_approve_assets(&harness, &harness.users.user, deposit_assets);

    let minted_shares = proxy_deposit(
        &harness,
        &harness.users.user,
        deposit_assets,
        &harness.users.user,
    );

    assert_eq!(minted_shares, deposit_assets);
    assert_eq!(
        share_client(&harness).balance(&harness.users.user),
        minted_shares
    );
    assert_eq!(proxy_total_assets(&harness), deposit_assets);
    assert_eq!(
        asset_client(&harness).balance(&harness.vault),
        deposit_assets
    );
}

#[test]
fn view_methods_match_vault_proxy_view() {
    let harness = setup_harness();
    let deposit_assets = 750_i128;
    let preview_assets = 123_i128;

    mint_and_approve_assets(&harness, &harness.users.user, deposit_assets);
    proxy_deposit(
        &harness,
        &harness.users.user,
        deposit_assets,
        &harness.users.user,
    );

    let vault_view = vault_proxy_fields(&harness, &harness.users.receiver, preview_assets, 0);
    let expected_total_assets = vault_view.core.totals.total_assets;
    let expected_convert_to_shares = vault_view.preview.convert_to_shares;
    let expected_max_deposit = vault_view.preview.max_deposit;

    assert_eq!(proxy_total_assets(&harness), expected_total_assets);
    assert_eq!(
        proxy_convert_to_shares(&harness, preview_assets),
        expected_convert_to_shares
    );
    assert_eq!(
        proxy_max_deposit(&harness, &harness.users.receiver),
        expected_max_deposit
    );
    assert_eq!(expected_max_deposit, i128::MAX);
}

#[test]
fn request_execute_withdraw_flow_burns_shares_and_returns_assets() {
    let harness = setup_harness();
    let deposit_assets = 1_200_i128;

    mint_and_approve_assets(&harness, &harness.users.user, deposit_assets);
    let minted_shares = proxy_deposit(
        &harness,
        &harness.users.user,
        deposit_assets,
        &harness.users.user,
    );
    let supply_after_deposit = vault_total_shares(&harness);

    let request_id = proxy_request_withdraw(
        &harness,
        &harness.users.user,
        &harness.users.receiver,
        minted_shares,
        0,
    );

    assert_eq!(request_id, 0);
    assert_eq!(share_client(&harness).balance(&harness.users.user), 0);
    assert_eq!(
        share_client(&harness).balance(&harness.vault),
        minted_shares
    );
    assert_eq!(vault_total_shares(&harness), supply_after_deposit);

    advance_past_cooldown(&harness.env);
    proxy_execute_withdraw(&harness, &harness.users.curator);

    assert_eq!(share_client(&harness).balance(&harness.vault), 0);
    assert_eq!(vault_total_shares(&harness), 0);
    assert_eq!(
        asset_client(&harness).balance(&harness.users.receiver),
        deposit_assets
    );
    assert_eq!(proxy_total_assets(&harness), 0);
}

#[test]
fn withdraw_flow_completes_queued_withdrawal() {
    let harness = setup_harness();
    let deposit_assets = 2_000_i128;
    let withdraw_assets = 1_200_i128;

    mint_and_approve_assets(&harness, &harness.users.user, deposit_assets);
    let minted_shares = proxy_deposit(
        &harness,
        &harness.users.user,
        deposit_assets,
        &harness.users.user,
    );
    let withdraw_shares = proxy_convert_to_shares(&harness, withdraw_assets);
    let request_id = proxy_withdraw(
        &harness,
        &harness.users.user,
        withdraw_assets,
        &harness.users.receiver,
        &harness.users.user,
    );

    assert_eq!(request_id, 0);
    assert_eq!(
        share_client(&harness).balance(&harness.users.user),
        minted_shares - withdraw_shares
    );
    assert_eq!(vault_total_shares(&harness), minted_shares);
    assert_eq!(
        share_client(&harness).balance(&harness.vault),
        withdraw_shares
    );
    assert_eq!(asset_client(&harness).balance(&harness.users.receiver), 0);
    assert_eq!(proxy_total_assets(&harness), deposit_assets);

    advance_past_cooldown(&harness.env);
    proxy_execute_withdraw(&harness, &harness.users.curator);

    assert_eq!(
        share_client(&harness).balance(&harness.users.user),
        minted_shares - withdraw_shares
    );
    assert_eq!(
        vault_total_shares(&harness),
        minted_shares - withdraw_shares
    );
    assert_eq!(
        asset_client(&harness).balance(&harness.users.receiver),
        withdraw_assets
    );
    assert_eq!(
        proxy_total_assets(&harness),
        deposit_assets - withdraw_assets
    );
}

#[test]
fn redeem_flow_completes_queued_withdrawal() {
    let harness = setup_harness();
    let deposit_assets = 2_000_i128;
    let redeem_shares = 1_200_i128;

    mint_and_approve_assets(&harness, &harness.users.user, deposit_assets);
    let minted_shares = proxy_deposit(
        &harness,
        &harness.users.user,
        deposit_assets,
        &harness.users.user,
    );
    let request_id = proxy_redeem(
        &harness,
        &harness.users.user,
        redeem_shares,
        &harness.users.receiver,
        &harness.users.user,
    );

    assert_eq!(request_id, 0);
    assert_eq!(
        share_client(&harness).balance(&harness.users.user),
        minted_shares - redeem_shares
    );
    assert_eq!(vault_total_shares(&harness), minted_shares);
    assert_eq!(
        share_client(&harness).balance(&harness.vault),
        redeem_shares
    );
    assert_eq!(asset_client(&harness).balance(&harness.users.receiver), 0);
    assert_eq!(proxy_total_assets(&harness), deposit_assets);

    advance_past_cooldown(&harness.env);
    proxy_execute_withdraw(&harness, &harness.users.curator);

    let redeemed_assets = proxy_convert_to_assets(&harness, redeem_shares);
    assert_eq!(
        share_client(&harness).balance(&harness.users.user),
        minted_shares - redeem_shares
    );
    assert_eq!(vault_total_shares(&harness), minted_shares - redeem_shares);
    assert_eq!(
        asset_client(&harness).balance(&harness.users.receiver),
        redeemed_assets
    );
    assert_eq!(
        proxy_total_assets(&harness),
        deposit_assets - redeemed_assets
    );
}
