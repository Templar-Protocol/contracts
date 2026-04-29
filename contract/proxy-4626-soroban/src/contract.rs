//! Core contract structure and helpers for the Soroban ERC-4626 proxy.

use alloc::string::String as AllocString;

use soroban_sdk::{
    contract, contractimpl, symbol_short, Address, Bytes, Env, IntoVal, InvokeError, Symbol,
};
use templar_soroban_shared_types::{
    DepositReceipt, ExecuteWithdrawReceipt, RequestWithdrawReceipt,
    VaultCommand as WireVaultCommand,
};

use crate::{error::ContractError, ProxyPreviewView, ProxyViewResponse};

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
    pub fn deposit(
        env: Env,
        caller: Address,
        assets: i128,
        receiver: Address,
    ) -> Result<i128, ContractError> {
        require_non_negative(assets)?;
        caller.require_auth();
        let receipt = decode_deposit_receipt(invoke_vault_execute(
            &env,
            VaultCommand::DepositWithMin {
                owner: caller.clone(),
                receiver: receiver.clone(),
                assets,
                min_shares_out: 0,
            },
        )?)?;
        let shares = receipt.shares_out;
        emit_deposit_event(&env, &caller, &receiver, assets, shares);
        Ok(shares)
    }

    pub fn mint(
        env: Env,
        caller: Address,
        shares: i128,
        receiver: Address,
    ) -> Result<i128, ContractError> {
        require_non_negative(shares)?;
        caller.require_auth();
        let preview = call_proxy_view(&env, &caller, 0, shares)?;
        let assets = preview.6;
        require_non_negative(assets)?;
        let receipt = decode_deposit_receipt(invoke_vault_execute(
            &env,
            VaultCommand::DepositWithMin {
                owner: caller.clone(),
                receiver: receiver.clone(),
                assets,
                min_shares_out: shares,
            },
        )?)?;
        emit_deposit_event(&env, &caller, &receiver, assets, receipt.shares_out);
        Ok(assets)
    }

    pub fn withdraw(
        env: Env,
        caller: Address,
        assets: i128,
        receiver: Address,
        owner: Address,
    ) -> Result<u64, ContractError> {
        require_non_negative(assets)?;
        let share_token = read_share_token(&env)?;
        let preview = call_proxy_view(&env, &owner, assets, 0)?;
        let shares = preview.7;
        require_non_negative(shares)?;
        require_auth_or_allowance(&env, &caller, &owner, &share_token, shares)?;
        let receipt = decode_request_withdraw_receipt(invoke_vault_execute(
            &env,
            VaultCommand::RequestWithdraw {
                owner: owner.clone(),
                receiver: receiver.clone(),
                shares,
                min_assets_out: assets,
            },
        )?)?;
        emit_redeem_request_event(
            &env,
            &receiver,
            &owner,
            receipt.request_id,
            &caller,
            receipt.shares_escrowed,
        );
        Ok(receipt.request_id)
    }

    pub fn redeem(
        env: Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
    ) -> Result<u64, ContractError> {
        require_non_negative(shares)?;
        let share_token = read_share_token(&env)?;
        require_auth_or_allowance(&env, &caller, &owner, &share_token, shares)?;
        let preview = call_proxy_view(&env, &owner, 0, shares)?;
        let assets = preview.1;
        require_non_negative(assets)?;
        let receipt = decode_request_withdraw_receipt(invoke_vault_execute(
            &env,
            VaultCommand::RequestWithdraw {
                owner: owner.clone(),
                receiver: receiver.clone(),
                shares,
                min_assets_out: assets,
            },
        )?)?;
        emit_redeem_request_event(
            &env,
            &receiver,
            &owner,
            receipt.request_id,
            &caller,
            receipt.shares_escrowed,
        );
        Ok(receipt.request_id)
    }

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
        let receipt = decode_request_withdraw_receipt(invoke_vault_execute(
            &env,
            VaultCommand::RequestWithdraw {
                owner: owner.clone(),
                receiver: receiver.clone(),
                shares,
                min_assets_out,
            },
        )?)?;
        emit_redeem_request_event(
            &env,
            &receiver,
            &owner,
            receipt.request_id,
            &owner,
            receipt.shares_escrowed,
        );
        Ok(receipt.request_id)
    }

    pub fn execute_withdraw(env: Env, caller: Address) -> Result<(), ContractError> {
        caller.require_auth();
        let receipt = decode_execute_withdraw_receipt(invoke_vault_execute(
            &env,
            VaultCommand::ExecuteWithdraw {
                caller: caller.clone(),
            },
        )?)?;
        if let ExecuteWithdrawReceipt::Completed {
            owner,
            receiver,
            assets_out,
            shares_burned,
            ..
        } = receipt
        {
            let owner = address_from_wire(&env, &owner)?;
            let receiver = address_from_wire(&env, &receiver)?;
            emit_withdraw_event(&env, &caller, &receiver, &owner, assets_out, shares_burned);
        }
        Ok(())
    }

    pub fn asset(env: Env) -> Result<Address, ContractError> {
        read_asset_token(&env)
    }

    pub fn total_assets(env: Env) -> Result<i128, ContractError> {
        let response = call_proxy_view_full(&env, &env.current_contract_address(), 0, 0)?;
        Ok(response.0 .2 .3)
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
        Ok(preview.0)
    }

    pub fn convert_to_assets(env: Env, shares: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), 0, shares)?;
        Ok(preview.1)
    }

    pub fn preview_deposit(env: Env, assets: i128) -> Result<i128, ContractError> {
        Self::convert_to_shares(env, assets)
    }

    pub fn preview_mint(env: Env, shares: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), 0, shares)?;
        Ok(preview.6)
    }

    pub fn preview_withdraw(env: Env, assets: i128) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &env.current_contract_address(), assets, 0)?;
        Ok(preview.7)
    }

    pub fn preview_redeem(env: Env, shares: i128) -> Result<i128, ContractError> {
        Self::convert_to_assets(env, shares)
    }

    pub fn max_deposit(env: Env, receiver: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &receiver, 0, 0)?;
        Ok(preview.2)
    }

    pub fn max_mint(env: Env, receiver: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &receiver, 0, 0)?;
        Ok(preview.3)
    }

    pub fn max_withdraw(env: Env, owner: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &owner, 0, 0)?;
        Ok(preview.4)
    }

    pub fn max_redeem(env: Env, owner: Address) -> Result<i128, ContractError> {
        let preview = call_proxy_view(&env, &owner, 0, 0)?;
        Ok(preview.5)
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
    is_initialized(env)
        .then_some(())
        .ok_or(ContractError::NotInitialized)
}

fn require_non_negative(amount: i128) -> Result<(), ContractError> {
    (amount >= 0)
        .then_some(())
        .ok_or(ContractError::InvalidInput)
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
) -> Result<Bytes, ContractError> {
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

    Ok(bytes)
}

fn call_proxy_view_full(
    env: &Env,
    owner: &Address,
    assets: i128,
    shares: i128,
) -> Result<ProxyViewResponse, ContractError> {
    let vault_address = read_vault_address(env)?;
    let proxy_view = Symbol::new(env, "proxy_view");
    let result = env.try_invoke_contract::<ProxyViewResponse, InvokeError>(
        &vault_address,
        &proxy_view,
        (owner.clone(), assets, shares).into_val(env),
    );

    match result {
        Ok(Ok(response)) => Ok(response),
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
) -> Result<ProxyPreviewView, ContractError> {
    let response = call_proxy_view_full(env, owner, assets, shares)?;
    let (_, _, preview) = response;
    Ok(preview)
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

fn decode_deposit_receipt(bytes: Bytes) -> Result<DepositReceipt, ContractError> {
    DepositReceipt::decode(&bytes.to_alloc_vec()).map_err(Into::into)
}

fn decode_request_withdraw_receipt(bytes: Bytes) -> Result<RequestWithdrawReceipt, ContractError> {
    RequestWithdrawReceipt::decode(&bytes.to_alloc_vec()).map_err(Into::into)
}

fn decode_execute_withdraw_receipt(bytes: Bytes) -> Result<ExecuteWithdrawReceipt, ContractError> {
    ExecuteWithdrawReceipt::decode(&bytes.to_alloc_vec()).map_err(Into::into)
}

pub(crate) fn require_auth_or_allowance(
    env: &Env,
    caller: &Address,
    owner: &Address,
    token: &Address,
    amount: i128,
) -> Result<(), ContractError> {
    if caller == owner {
        owner.require_auth();
        return Ok(());
    }

    caller.require_auth();
    let proxy = env.current_contract_address();
    let allowance: i128 =
        call_token_view_with_two_addresses(env, token, "allowance", owner, &proxy)?;

    (allowance >= amount)
        .then_some(())
        .ok_or(ContractError::InsufficientAllowance)
}

fn call_token_view_with_two_addresses<T>(
    env: &Env,
    token: &Address,
    method: &str,
    first: &Address,
    second: &Address,
) -> Result<T, ContractError>
where
    T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
{
    map_token_invoke_result(env.try_invoke_contract::<T, soroban_sdk::Error>(
        token,
        &Symbol::new(env, method),
        soroban_sdk::vec![env, first.into_val(env), second.into_val(env)],
    ))
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

#[allow(deprecated)]
pub(crate) fn emit_withdraw_event(
    env: &Env,
    sender: &Address,
    receiver: &Address,
    owner: &Address,
    assets: i128,
    shares: i128,
) {
    env.events().publish(
        (
            symbol_short!("Withdraw"),
            sender.clone(),
            receiver.clone(),
            owner.clone(),
        ),
        (assets, shares),
    );
}

fn address_to_wire(address: &Address) -> Result<AllocString, ContractError> {
    let raw = address.to_string().to_bytes().to_alloc_vec();
    AllocString::from_utf8(raw).map_err(|_| ContractError::InvalidInput)
}

fn address_from_wire(env: &Env, value: &AllocString) -> Result<Address, ContractError> {
    validate_address_strkey(value.as_bytes())?;
    Ok(Address::from_str(env, value))
}

fn validate_address_strkey(bytes: &[u8]) -> Result<(), ContractError> {
    const STRKEY_LEN: usize = 56;
    const BINARY_LEN: usize = 35;
    const ACCOUNT_VERSION: u8 = 6 << 3;
    const CONTRACT_VERSION: u8 = 2 << 3;

    if bytes.len() != STRKEY_LEN {
        return Err(ContractError::InvalidInput);
    }

    let mut out = [0u8; BINARY_LEN];
    let mut buffer = 0u16;
    let mut bits = 0u8;
    let mut cursor = 0usize;
    for byte in bytes {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => return Err(ContractError::InvalidInput),
        };
        buffer = (buffer << 5) | u16::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if cursor >= BINARY_LEN {
                return Err(ContractError::InvalidInput);
            }
            out[cursor] = (buffer >> bits) as u8;
            cursor += 1;
            buffer &= (1u16 << bits) - 1;
        }
    }

    if cursor != BINARY_LEN
        || bits != 0
        || (out[0] != ACCOUNT_VERSION && out[0] != CONTRACT_VERSION)
    {
        return Err(ContractError::InvalidInput);
    }

    let expected = u16::from_le_bytes([out[BINARY_LEN - 2], out[BINARY_LEN - 1]]);
    let actual = crc16_xmodem(&out[..BINARY_LEN - 2]);
    if expected != actual {
        return Err(ContractError::InvalidInput);
    }

    Ok(())
}

fn crc16_xmodem(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 == 0 {
                crc <<= 1;
            } else {
                crc = (crc << 1) ^ 0x1021;
            }
        }
    }
    crc
}
