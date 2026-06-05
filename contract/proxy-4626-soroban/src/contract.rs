//! Core contract structure and helpers for the Soroban ERC-4626 proxy.
//!
//! The proxy uses Soroban-style explicit `operator` arguments instead of an
//! ambient `msg.sender`. For deposits, the operator is also the asset source.
//! For asynchronous redemptions, the current audited compatibility path is
//! owner-operated only: `operator == owner`.
//!
//! This deliberately rejects EIP-7540 / OpenZeppelin-style delegated redemption
//! operators for now. A proper delegated path would need to add `operator` to
//! the vault request command, thread it through the vault and kernel request
//! path, escrow shares with `transfer_from` when `operator != owner`, consume
//! allowance correctly, and test the flow without mocked authorization. That is
//! useful future UX for routers, custodians, multisigs, batched executors, and
//! market-maker infrastructure, but it changes share-movement authority and
//! should be treated as a separately reviewed/audited change.

use alloc::string::String as AllocString;

use soroban_sdk::{
    contract, contractimpl, symbol_short, Address, Bytes, Env, IntoVal, InvokeError, Symbol,
};
use templar_soroban_shared_types::{
    ProxyPreviewFields, ProxyViewFields, ProxyViewResponse, VaultCommand as WireVaultCommand,
    VaultCommandResult as WireVaultCommandResult,
};

use crate::error::ContractError;

const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;

#[derive(Clone, Eq, PartialEq)]
pub(crate) enum VaultCommand {
    DepositWithMin {
        owner: Address,
        receiver: Address,
        assets: i128,
        min_shares_out: i128,
    },
    RequestWithdraw {
        owner: Address,
        receiver: Address,
        shares: i128,
        min_assets_out: i128,
    },
    ExecuteWithdraw {
        caller: Address,
    },
}

impl VaultCommand {
    fn into_wire(self) -> Result<WireVaultCommand, ContractError> {
        match self {
            Self::DepositWithMin {
                owner,
                receiver,
                assets,
                min_shares_out,
            } => Ok(WireVaultCommand::DepositWithMin {
                owner: address_to_wire(&owner)?,
                receiver: address_to_wire(&receiver)?,
                assets,
                min_shares_out,
            }),
            Self::RequestWithdraw {
                owner,
                receiver,
                shares,
                min_assets_out,
            } => Ok(WireVaultCommand::RequestWithdraw {
                owner: address_to_wire(&owner)?,
                receiver: address_to_wire(&receiver)?,
                shares,
                min_assets_out,
            }),
            Self::ExecuteWithdraw { caller } => Ok(WireVaultCommand::ExecuteWithdraw {
                caller: address_to_wire(&caller)?,
            }),
        }
    }
}

/// Internal storage keys for proxy config.
#[allow(non_upper_case_globals)]
pub struct ProxyDataKey;

#[allow(non_upper_case_globals)]
impl ProxyDataKey {
    pub const VaultAddress: Symbol = symbol_short!("vault");
    pub const AssetToken: Symbol = symbol_short!("asset");
    pub const ShareToken: Symbol = symbol_short!("share");
    pub const Initialized: Symbol = symbol_short!("init");
}

#[contract]
pub struct Soroban4626ProxyContract;

#[contractimpl]
impl Soroban4626ProxyContract {
    /// Synchronous asset deposit into the underlying vault.
    ///
    /// `operator` is explicit because Soroban has no ambient `msg.sender`; the
    /// proxy must know which address to authenticate with `require_auth()`.
    /// This method follows the ERC-4626 deposit shape, but with the Soroban
    /// operator address passed as an argument. The operator is also the asset
    /// source for this compatibility entrypoint.
    pub fn deposit(
        env: Env,
        operator: Address,
        assets: i128,
        receiver: Address,
    ) -> Result<i128, ContractError> {
        deposit_with_min_internal(env, operator, assets, receiver, 0)
    }

    /// Synchronous asset deposit with explicit minimum shares out.
    ///
    /// This exposes the vault's native `DepositWithMin` slippage guard through
    /// the proxy. `operator` is authenticated and is also the asset source for
    /// this compatibility entrypoint.
    pub fn deposit_with_min(
        env: Env,
        operator: Address,
        assets: i128,
        receiver: Address,
        min_shares_out: i128,
    ) -> Result<i128, ContractError> {
        deposit_with_min_internal(env, operator, assets, receiver, min_shares_out)
    }

    /// Synchronous share mint into the underlying vault.
    ///
    /// `operator` is explicit because Soroban has no ambient `msg.sender`; the
    /// proxy must know which address to authenticate with `require_auth()`.
    /// The proxy previews the assets needed and submits a minimum share output
    /// equal to `shares`. The operator is also the asset source for this
    /// compatibility entrypoint.
    pub fn mint(
        env: Env,
        operator: Address,
        shares: i128,
        receiver: Address,
    ) -> Result<i128, ContractError> {
        require_non_negative(shares)?;
        operator.require_auth();
        let preview = call_proxy_view(&env, &operator, 0, shares)?;
        let assets = preview.preview_mint_assets;
        require_non_negative(assets)?;
        let minted_shares = expect_i128_result(invoke_vault_execute(
            &env,
            VaultCommand::DepositWithMin {
                owner: operator.clone(),
                receiver: receiver.clone(),
                assets,
                min_shares_out: shares,
            },
        )?)?;
        emit_deposit_event(&env, &operator, &receiver, assets, minted_shares);
        Ok(assets)
    }

    /// Request an asynchronous redemption by asset amount.
    ///
    /// This is an ERC-7540-style redemption request, not a synchronous
    /// ERC-4626 withdrawal. It escrows shares in the vault and returns a
    /// `request_id`; assets are not transferred to `receiver` until the
    /// withdrawal becomes executable and `execute_withdraw` succeeds.
    ///
    /// `operator` is explicit because Soroban has no ambient `msg.sender`.
    /// The supported path is `operator == owner`. Operator-style delegated
    /// requests require a transfer-from-backed vault request path.
    pub fn withdraw(
        env: Env,
        operator: Address,
        assets: i128,
        receiver: Address,
        owner: Address,
    ) -> Result<u64, ContractError> {
        require_non_negative(assets)?;
        operator.require_auth();
        require_self_operator(&operator, &owner)?;
        let preview = call_proxy_view(&env, &owner, assets, 0)?;
        let shares = preview.preview_withdraw_shares;
        require_non_negative(shares)?;
        let request_id = expect_u64_result(invoke_vault_execute(
            &env,
            VaultCommand::RequestWithdraw {
                owner: owner.clone(),
                receiver: receiver.clone(),
                shares,
                min_assets_out: assets,
            },
        )?)?;
        emit_redeem_request_event(&env, &receiver, &owner, request_id, &operator, shares);
        Ok(request_id)
    }

    /// Request an asynchronous redemption by share amount.
    ///
    /// This is an ERC-7540-style redemption request, not a synchronous
    /// ERC-4626 redeem. It escrows `shares` in the vault and returns a
    /// `request_id`; assets are not transferred to `receiver` until the
    /// withdrawal becomes executable and `execute_withdraw` succeeds.
    ///
    /// `operator` is explicit because Soroban has no ambient `msg.sender`.
    /// The supported path is `operator == owner`. Operator-style delegated
    /// requests require a transfer-from-backed vault request path.
    pub fn redeem(
        env: Env,
        operator: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
    ) -> Result<u64, ContractError> {
        require_non_negative(shares)?;
        operator.require_auth();
        require_self_operator(&operator, &owner)?;
        let preview = call_proxy_view(&env, &owner, 0, shares)?;
        let assets = preview.convert_to_assets;
        require_non_negative(assets)?;
        let request_id = expect_u64_result(invoke_vault_execute(
            &env,
            VaultCommand::RequestWithdraw {
                owner: owner.clone(),
                receiver: receiver.clone(),
                shares,
                min_assets_out: assets,
            },
        )?)?;
        emit_redeem_request_event(&env, &receiver, &owner, request_id, &operator, shares);
        Ok(request_id)
    }

    /// Lower-level asynchronous redemption request with explicit slippage.
    ///
    /// This mirrors the vault's native request path: `owner` authenticates,
    /// `shares` are escrowed, and the returned `request_id` identifies the
    /// queued redemption request. Claiming assets is a separate
    /// `execute_withdraw` step after cooldown/liquidity conditions are met.
    pub fn request_withdraw(
        env: Env,
        owner: Address,
        receiver: Address,
        shares: i128,
        min_assets_out: i128,
    ) -> Result<u64, ContractError> {
        require_non_negative(shares)?;
        require_non_negative(min_assets_out)?;
        owner.require_auth();
        let request_id = expect_u64_result(invoke_vault_execute(
            &env,
            VaultCommand::RequestWithdraw {
                owner: owner.clone(),
                receiver: receiver.clone(),
                shares,
                min_assets_out,
            },
        )?)?;
        emit_redeem_request_event(&env, &receiver, &owner, request_id, &owner, shares);
        Ok(request_id)
    }

    /// Execute the next claimable queued withdrawal.
    ///
    /// `operator` is the authenticated vault executor/keeper, not a request id.
    /// This method executes according to the vault's queue and authorization
    /// policy; it does not select a withdrawal request by `request_id`.
    pub fn execute_withdraw(env: Env, operator: Address) -> Result<(), ContractError> {
        operator.require_auth();
        expect_unit_result(invoke_vault_execute(
            &env,
            VaultCommand::ExecuteWithdraw { caller: operator },
        )?)
    }

    pub fn asset(env: Env) -> Result<Address, ContractError> {
        read_asset_token(&env)
    }

    pub fn total_assets(env: Env) -> Result<i128, ContractError> {
        let response = call_proxy_view_full(&env, &env.current_contract_address(), 0, 0)?;
        Ok(response.core.totals.total_assets)
    }

    pub fn total_supply(env: Env) -> Result<i128, ContractError> {
        let share_token = read_share_token(&env)?;
        call_token_view_no_args(&env, &share_token, "total_supply")
    }

    pub fn balance_of(env: Env, owner: Address) -> Result<i128, ContractError> {
        let share_token = read_share_token(&env)?;
        call_token_view_with_address(&env, &share_token, "balance", &owner)
    }

    pub fn convert_to_shares(env: Env, assets: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), assets, 0)?;
        Ok(preview.convert_to_shares)
    }

    pub fn convert_to_assets(env: Env, shares: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), 0, shares)?;
        Ok(preview.convert_to_assets)
    }

    pub fn preview_deposit(env: Env, assets: i128) -> Result<i128, ContractError> {
        Self::convert_to_shares(env, assets)
    }

    pub fn preview_mint(env: Env, shares: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), 0, shares)?;
        Ok(preview.preview_mint_assets)
    }

    pub fn preview_withdraw(env: Env, assets: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), assets, 0)?;
        Ok(preview.preview_withdraw_shares)
    }

    pub fn preview_redeem(env: Env, shares: i128) -> Result<i128, ContractError> {
        Self::convert_to_assets(env, shares)
    }

    pub fn max_deposit(env: Env, receiver: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &receiver, 0, 0)?;
        Ok(preview.max_deposit)
    }

    pub fn max_mint(env: Env, receiver: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &receiver, 0, 0)?;
        Ok(preview.max_mint)
    }

    pub fn max_withdraw(env: Env, owner: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &owner, 0, 0)?;
        Ok(preview.max_withdraw)
    }

    pub fn max_redeem(env: Env, owner: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &owner, 0, 0)?;
        Ok(preview.max_redeem)
    }

    pub fn decimals(env: Env) -> Result<u32, ContractError> {
        let share_token = read_share_token(&env)?;
        call_token_view_no_args(&env, &share_token, "decimals")
    }

    pub fn name(env: Env) -> Result<soroban_sdk::String, ContractError> {
        let share_token = read_share_token(&env)?;
        call_token_view_no_args(&env, &share_token, "name")
    }

    pub fn symbol(env: Env) -> Result<soroban_sdk::String, ContractError> {
        let share_token = read_share_token(&env)?;
        call_token_view_no_args(&env, &share_token, "symbol")
    }

    pub fn initialize(
        env: Env,
        vault_address: Address,
        asset_token: Address,
        share_token: Address,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        if is_initialized(&env) {
            return Err(ContractError::AlreadyInitialized);
        }

        env.storage()
            .instance()
            .set(&ProxyDataKey::VaultAddress, &vault_address);
        env.storage()
            .instance()
            .set(&ProxyDataKey::AssetToken, &asset_token);
        env.storage()
            .instance()
            .set(&ProxyDataKey::ShareToken, &share_token);
        env.storage()
            .instance()
            .set(&ProxyDataKey::Initialized, &true);
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Extend proxy configuration TTL.
    ///
    /// This is permissionless because it only preserves existing proxy config;
    /// it cannot mutate vault accounting or authorization state.
    pub fn extend_ttl(env: Env) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        Ok(())
    }
}

pub(crate) fn is_initialized(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&ProxyDataKey::Initialized)
        .unwrap_or(false)
}

pub(crate) fn require_initialized(env: &Env) -> Result<(), ContractError> {
    extend_instance_ttl(env);
    is_initialized(env)
        .then_some(())
        .ok_or(ContractError::NotInitialized)
}

fn require_non_negative(amount: i128) -> Result<(), ContractError> {
    (amount >= 0)
        .then_some(())
        .ok_or(ContractError::InvalidInput)
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

fn deposit_with_min_internal(
    env: Env,
    operator: Address,
    assets: i128,
    receiver: Address,
    min_shares_out: i128,
) -> Result<i128, ContractError> {
    require_non_negative(assets)?;
    require_non_negative(min_shares_out)?;
    operator.require_auth();
    let shares = expect_i128_result(invoke_vault_execute(
        &env,
        VaultCommand::DepositWithMin {
            owner: operator.clone(),
            receiver: receiver.clone(),
            assets,
            min_shares_out,
        },
    )?)?;
    emit_deposit_event(&env, &operator, &receiver, assets, shares);
    Ok(shares)
}

pub(crate) fn read_vault_address(env: &Env) -> Result<Address, ContractError> {
    require_initialized(env)?;
    env.storage()
        .instance()
        .get(&ProxyDataKey::VaultAddress)
        .ok_or(ContractError::NotInitialized)
}

pub(crate) fn read_asset_token(env: &Env) -> Result<Address, ContractError> {
    require_initialized(env)?;
    env.storage()
        .instance()
        .get(&ProxyDataKey::AssetToken)
        .ok_or(ContractError::NotInitialized)
}

pub(crate) fn read_share_token(env: &Env) -> Result<Address, ContractError> {
    require_initialized(env)?;
    env.storage()
        .instance()
        .get(&ProxyDataKey::ShareToken)
        .ok_or(ContractError::NotInitialized)
}

pub(crate) fn invoke_vault_execute(
    env: &Env,
    command: VaultCommand,
) -> Result<WireVaultCommandResult, ContractError> {
    let vault_address = read_vault_address(env)?;
    let command = command.into_wire()?;
    let payload = Bytes::from_slice(env, &command.encode());
    let execute = Symbol::new(env, "execute");

    let result = env.try_invoke_contract::<Bytes, InvokeError>(
        &vault_address,
        &execute,
        (&payload,).into_val(env),
    );

    let bytes = match result {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return Err(ContractError::VaultError),
        Err(Ok(invoke_error)) => return Err(map_vault_invoke_error(invoke_error)),
        Err(Err(invoke_error)) => return Err(map_vault_invoke_error(invoke_error)),
    };

    WireVaultCommandResult::decode(&bytes.to_alloc_vec()).map_err(Into::into)
}

fn call_proxy_view_full(
    env: &Env,
    owner: &Address,
    assets: i128,
    shares: i128,
) -> Result<ProxyViewFields, ContractError> {
    let vault_address = read_vault_address(env)?;
    let proxy_view = Symbol::new(env, "proxy_view");
    let result = env.try_invoke_contract::<ProxyViewResponse, InvokeError>(
        &vault_address,
        &proxy_view,
        (owner.clone(), assets, shares).into_val(env),
    );

    match result {
        Ok(Ok(response)) => Ok(response.into()),
        Ok(Err(_)) => Err(ContractError::VaultError),
        Err(Ok(invoke_error)) => Err(map_vault_invoke_error(invoke_error)),
        Err(Err(invoke_error)) => Err(map_vault_invoke_error(invoke_error)),
    }
}

fn map_vault_invoke_error(error: InvokeError) -> ContractError {
    match error {
        InvokeError::Abort => ContractError::VaultError,
        InvokeError::Contract(code) => ContractError::from_vault_error_code(code),
    }
}

fn call_proxy_view(
    env: &Env,
    owner: &Address,
    assets: i128,
    shares: i128,
) -> Result<ProxyPreviewFields, ContractError> {
    let response = call_proxy_view_full(env, owner, assets, shares)?;
    Ok(response.preview)
}

fn call_token_view_no_args<T>(env: &Env, token: &Address, method: &str) -> Result<T, ContractError>
where
    T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
{
    map_token_invoke_result(env.try_invoke_contract::<T, soroban_sdk::Error>(
        token,
        &Symbol::new(env, method),
        soroban_sdk::vec![env],
    ))
}

fn call_token_view_with_address<T>(
    env: &Env,
    token: &Address,
    method: &str,
    address: &Address,
) -> Result<T, ContractError>
where
    T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
{
    map_token_invoke_result(env.try_invoke_contract::<T, soroban_sdk::Error>(
        token,
        &Symbol::new(env, method),
        soroban_sdk::vec![env, address.into_val(env)],
    ))
}

fn map_token_invoke_result<T>(
    result: Result<Result<T, T::Error>, Result<soroban_sdk::Error, InvokeError>>,
) -> Result<T, ContractError>
where
    T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
{
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(ContractError::VaultError),
        Err(Ok(_)) => Err(ContractError::VaultError),
        Err(Err(InvokeError::Abort | InvokeError::Contract(_))) => Err(ContractError::VaultError),
    }
}

fn expect_i128_result(result: WireVaultCommandResult) -> Result<i128, ContractError> {
    match result {
        WireVaultCommandResult::I128(value) => Ok(value),
        _ => Err(ContractError::VaultError),
    }
}

fn expect_u64_result(result: WireVaultCommandResult) -> Result<u64, ContractError> {
    match result {
        WireVaultCommandResult::U64(value) => Ok(value),
        _ => Err(ContractError::VaultError),
    }
}

fn expect_unit_result(result: WireVaultCommandResult) -> Result<(), ContractError> {
    match result {
        WireVaultCommandResult::Unit | WireVaultCommandResult::ExecuteWithdrawStatus(_) => Ok(()),
        _ => Err(ContractError::VaultError),
    }
}

fn require_self_operator(operator: &Address, owner: &Address) -> Result<(), ContractError> {
    (operator == owner)
        .then_some(())
        .ok_or(ContractError::InsufficientAllowance)
}

#[allow(deprecated)]
pub(crate) fn emit_deposit_event(
    env: &Env,
    caller: &Address,
    owner: &Address,
    assets: i128,
    shares: i128,
) {
    env.events().publish(
        (symbol_short!("Deposit"), caller.clone(), owner.clone()),
        (assets, shares),
    );
}

#[allow(deprecated)]
pub(crate) fn emit_redeem_request_event(
    env: &Env,
    controller: &Address,
    owner: &Address,
    request_id: u64,
    sender: &Address,
    shares: i128,
) {
    env.events().publish(
        (
            Symbol::new(env, "RedeemRequest"),
            controller.clone(),
            owner.clone(),
            request_id,
        ),
        (sender.clone(), shares),
    );
}

fn address_to_wire(address: &Address) -> Result<AllocString, ContractError> {
    let raw = address.to_string().to_bytes().to_alloc_vec();
    AllocString::from_utf8(raw).map_err(|_| ContractError::InvalidInput)
}
