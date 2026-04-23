//! Typed curator and governance operations facade for the Soroban vault.

use alloc::string::String as AllocString;

use soroban_sdk::{
    contract, contractimpl, symbol_short, Address, Bytes, BytesN, Env, IntoVal, InvokeError,
    String, Symbol, TryFromVal, Val, Vec,
};
use templar_soroban_governance::{
    GovernanceActionKind, GovernanceError, PendingProposal, TimelockKind, Timelocks,
};
use templar_soroban_shared_types::{
    VaultCommand as WireVaultCommand, VaultCommandResult as WireVaultCommandResult,
};

use crate::error::ContractError;

#[derive(Clone, Eq, PartialEq)]
pub(crate) enum VaultCommand {
    Allocate {
        caller: Address,
        market: u32,
        amount: i128,
        supply: bool,
    },
    RefreshMarkets {
        caller: Address,
        markets: Vec<u32>,
    },
    ResyncIdleBalance,
    CancelMigration {
        caller: Address,
    },
    ExtendTtl,
}

impl VaultCommand {
    fn into_wire(self) -> Result<WireVaultCommand, ContractError> {
        match self {
            Self::Allocate {
                caller,
                market,
                amount,
                supply,
            } => Ok(WireVaultCommand::Allocate {
                caller: address_to_wire(&caller)?,
                market,
                amount,
                supply,
            }),
            Self::RefreshMarkets { caller, markets } => Ok(WireVaultCommand::RefreshMarkets {
                caller: address_to_wire(&caller)?,
                markets: soroban_u32_vec_to_alloc(markets),
            }),
            Self::ResyncIdleBalance => Ok(WireVaultCommand::ResyncIdleBalance),
            Self::CancelMigration { caller } => Ok(WireVaultCommand::CancelMigration {
                caller: address_to_wire(&caller)?,
            }),
            Self::ExtendTtl => Ok(WireVaultCommand::ExtendTtl),
        }
    }
}

#[allow(non_upper_case_globals)]
pub struct ProxyDataKey;

#[allow(non_upper_case_globals)]
impl ProxyDataKey {
    pub const VaultAddress: Symbol = symbol_short!("vault");
    pub const GovernanceAddress: Symbol = symbol_short!("gov");
    pub const Initialized: Symbol = symbol_short!("init");
}

#[contract]
pub struct SorobanCuratorProxyContract;

#[contractimpl]
impl SorobanCuratorProxyContract {
    pub fn initialize(
        env: Env,
        vault_address: Address,
        governance_address: Address,
    ) -> Result<(), ContractError> {
        if is_initialized(&env) {
            return Err(ContractError::AlreadyInitialized);
        }

        env.storage()
            .instance()
            .set(&ProxyDataKey::VaultAddress, &vault_address);
        env.storage()
            .instance()
            .set(&ProxyDataKey::GovernanceAddress, &governance_address);
        env.storage()
            .instance()
            .set(&ProxyDataKey::Initialized, &true);
        Ok(())
    }

    pub fn vault(env: Env) -> Result<Address, ContractError> {
        read_vault_address(&env)
    }

    pub fn governance(env: Env) -> Result<Address, ContractError> {
        read_governance_address(&env)
    }

    pub fn supply_market(
        env: Env,
        caller: Address,
        market: u32,
        amount: i128,
    ) -> Result<i128, ContractError> {
        caller.require_auth();
        expect_i128_result(invoke_vault_execute(
            &env,
            VaultCommand::Allocate {
                caller,
                market,
                amount,
                supply: true,
            },
        )?)
    }

    pub fn withdraw_market(
        env: Env,
        caller: Address,
        market: u32,
        amount: i128,
    ) -> Result<i128, ContractError> {
        caller.require_auth();
        expect_i128_result(invoke_vault_execute(
            &env,
            VaultCommand::Allocate {
                caller,
                market,
                amount,
                supply: false,
            },
        )?)
    }

    pub fn refresh_markets(
        env: Env,
        caller: Address,
        markets: Vec<u32>,
    ) -> Result<i128, ContractError> {
        caller.require_auth();
        expect_i128_result(invoke_vault_execute(
            &env,
            VaultCommand::RefreshMarkets { caller, markets },
        )?)
    }

    pub fn resync_idle_balance(env: Env) -> Result<(), ContractError> {
        expect_unit_result(invoke_vault_execute(&env, VaultCommand::ResyncIdleBalance)?)
    }

    pub fn extend_vault_ttl(env: Env) -> Result<(), ContractError> {
        expect_unit_result(invoke_vault_execute(&env, VaultCommand::ExtendTtl)?)
    }

    pub fn cancel_migration(env: Env, caller: Address) -> Result<(), ContractError> {
        caller.require_auth();
        expect_unit_result(invoke_vault_execute(
            &env,
            VaultCommand::CancelMigration { caller },
        )?)
    }

    pub fn submit_set_paused(
        env: Env,
        caller: Address,
        paused: bool,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(&env, "submit_set_paused", (caller, paused).into_val(&env))
    }

    pub fn submit_set_curator(
        env: Env,
        caller: Address,
        new_curator: Address,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_curator",
            (caller, new_curator).into_val(&env),
        )
    }

    pub fn submit_set_governance(
        env: Env,
        caller: Address,
        governance: Address,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_governance",
            (caller, governance).into_val(&env),
        )
    }

    pub fn submit_set_supply_queue(
        env: Env,
        caller: Address,
        target_ids: Vec<u32>,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_supply_queue",
            (caller, target_ids).into_val(&env),
        )
    }

    pub fn submit_set_fees(
        env: Env,
        caller: Address,
        performance_fee_wad: i128,
        performance_recipient: Address,
        management_fee_wad: i128,
        management_recipient: Address,
        max_growth_rate_wad: Option<i128>,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_fees",
            (
                caller,
                performance_fee_wad,
                performance_recipient,
                management_fee_wad,
                management_recipient,
                max_growth_rate_wad,
            )
                .into_val(&env),
        )
    }

    pub fn submit_set_restrictions(
        env: Env,
        caller: Address,
        mode: u32,
        accounts: Vec<Address>,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_restrictions",
            (caller, mode, accounts).into_val(&env),
        )
    }

    pub fn submit_set_guardian(
        env: Env,
        caller: Address,
        guardian: Address,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_guardian",
            (caller, guardian).into_val(&env),
        )
    }

    pub fn submit_set_sentinel(
        env: Env,
        caller: Address,
        sentinel: Address,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_sentinel",
            (caller, sentinel).into_val(&env),
        )
    }

    pub fn submit_set_timelock(
        env: Env,
        caller: Address,
        kind: TimelockKind,
        new_timelock_ns: u64,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_timelock",
            (caller, kind, new_timelock_ns).into_val(&env),
        )
    }

    pub fn submit_set_cap(
        env: Env,
        caller: Address,
        market_id: u32,
        new_cap: i128,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_cap",
            (caller, market_id, new_cap).into_val(&env),
        )
    }

    pub fn submit_remove_market(
        env: Env,
        caller: Address,
        market_id: u32,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_remove_market",
            (caller, market_id).into_val(&env),
        )
    }

    pub fn submit_set_group_cap(
        env: Env,
        caller: Address,
        cap_group_id: String,
        new_cap: i128,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_group_cap",
            (caller, cap_group_id, new_cap).into_val(&env),
        )
    }

    pub fn submit_set_group_rel_cap(
        env: Env,
        caller: Address,
        cap_group_id: String,
        new_relative_cap_wad: i128,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_group_rel_cap",
            (caller, cap_group_id, new_relative_cap_wad).into_val(&env),
        )
    }

    pub fn submit_set_group_member(
        env: Env,
        caller: Address,
        market_id: u32,
        cap_group_id: String,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_group_member",
            (caller, market_id, cap_group_id).into_val(&env),
        )
    }

    pub fn submit_set_skim_recipient(
        env: Env,
        caller: Address,
        recipient: Address,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_set_skim_recipient",
            (caller, recipient).into_val(&env),
        )
    }

    pub fn submit_skim(env: Env, caller: Address, token: Address) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(&env, "submit_skim", (caller, token).into_val(&env))
    }

    pub fn submit_other(
        env: Env,
        caller: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "submit_other",
            (caller, key, payload_hash).into_val(&env),
        )
    }

    pub fn accept(env: Env, caller: Address, proposal_id: u64) -> Result<(), ContractError> {
        caller.require_auth();
        invoke_governance(&env, "accept", (caller, proposal_id).into_val(&env))
    }

    pub fn accept_kind(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<u64, ContractError> {
        caller.require_auth();
        invoke_governance(&env, "accept_kind", (caller, kind).into_val(&env))
    }

    pub fn revoke(env: Env, caller: Address, proposal_id: u64) -> Result<(), ContractError> {
        caller.require_auth();
        invoke_governance(&env, "revoke", (caller, proposal_id).into_val(&env))
    }

    pub fn revoke_kind(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<u32, ContractError> {
        caller.require_auth();
        invoke_governance(&env, "revoke_kind", (caller, kind).into_val(&env))
    }

    pub fn revoke_other_pending(
        env: Env,
        caller: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<u32, ContractError> {
        caller.require_auth();
        invoke_governance(
            &env,
            "revoke_other_pending",
            (caller, key, payload_hash).into_val(&env),
        )
    }

    pub fn abdicate(
        env: Env,
        caller: Address,
        kind: GovernanceActionKind,
    ) -> Result<(), ContractError> {
        caller.require_auth();
        invoke_governance(&env, "abdicate", (caller, kind).into_val(&env))
    }

    pub fn pending(env: Env, proposal_id: u64) -> Result<PendingProposal, ContractError> {
        invoke_governance(&env, "pending", (proposal_id,).into_val(&env))
    }

    pub fn pending_ids(env: Env) -> Result<Vec<u64>, ContractError> {
        invoke_governance(&env, "pending_ids", Vec::new(&env))
    }

    pub fn timelock_ns(env: Env, kind: TimelockKind) -> Result<u64, ContractError> {
        invoke_governance(&env, "timelock_ns", (kind,).into_val(&env))
    }

    pub fn timelocks(env: Env) -> Result<Timelocks, ContractError> {
        invoke_governance(&env, "timelocks", Vec::new(&env))
    }

    pub fn admin(env: Env) -> Result<Address, ContractError> {
        invoke_governance(&env, "admin", Vec::new(&env))
    }

    pub fn guardian(env: Env) -> Result<Option<Address>, ContractError> {
        invoke_governance(&env, "guardian", Vec::new(&env))
    }

    pub fn sentinel(env: Env) -> Result<Option<Address>, ContractError> {
        invoke_governance(&env, "sentinel", Vec::new(&env))
    }

    pub fn is_abdicated(env: Env, kind: GovernanceActionKind) -> Result<bool, ContractError> {
        invoke_governance(&env, "is_abdicated", (kind,).into_val(&env))
    }

    pub fn check_other(
        env: Env,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<bool, ContractError> {
        invoke_governance(&env, "check_other", (key, payload_hash).into_val(&env))
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

pub(crate) fn read_vault_address(env: &Env) -> Result<Address, ContractError> {
    require_initialized(env)?;
    env.storage()
        .instance()
        .get(&ProxyDataKey::VaultAddress)
        .ok_or(ContractError::NotInitialized)
}

pub(crate) fn read_governance_address(env: &Env) -> Result<Address, ContractError> {
    require_initialized(env)?;
    env.storage()
        .instance()
        .get(&ProxyDataKey::GovernanceAddress)
        .ok_or(ContractError::NotInitialized)
}

pub(crate) fn invoke_vault_execute(
    env: &Env,
    command: VaultCommand,
) -> Result<WireVaultCommandResult, ContractError> {
    let vault_address = read_vault_address(env)?;
    let command = command.into_wire()?;
    let payload = Bytes::from_slice(env, &command.encode());
    let result = env.try_invoke_contract::<Bytes, ContractError>(
        &vault_address,
        &Symbol::new(env, "execute"),
        (&payload,).into_val(env),
    );

    let bytes = match result {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return Err(ContractError::VaultError),
        Err(Ok(error)) => return Err(error),
        Err(Err(invoke_error)) => {
            return Err(match invoke_error {
                InvokeError::Abort => ContractError::VaultError,
                InvokeError::Contract(code) => ContractError::from_vault_error_code(code),
            })
        }
    };

    WireVaultCommandResult::decode(&bytes.to_alloc_vec()).map_err(Into::into)
}

fn invoke_governance<T>(env: &Env, method: &str, args: Vec<Val>) -> Result<T, ContractError>
where
    T: TryFromVal<Env, Val>,
{
    let governance_address = read_governance_address(env)?;
    match env.try_invoke_contract::<T, GovernanceError>(
        &governance_address,
        &Symbol::new(env, method),
        args,
    ) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) | Err(Ok(_)) => Err(ContractError::GovernanceError),
        Err(Err(InvokeError::Abort | InvokeError::Contract(_))) => {
            Err(ContractError::GovernanceError)
        }
    }
}

fn expect_i128_result(result: WireVaultCommandResult) -> Result<i128, ContractError> {
    match result {
        WireVaultCommandResult::I128(value) => Ok(value),
        _ => Err(ContractError::VaultError),
    }
}

fn expect_unit_result(result: WireVaultCommandResult) -> Result<(), ContractError> {
    match result {
        WireVaultCommandResult::Unit => Ok(()),
        _ => Err(ContractError::VaultError),
    }
}

fn address_to_wire(address: &Address) -> Result<AllocString, ContractError> {
    let raw = address.to_string().to_bytes().to_alloc_vec();
    AllocString::from_utf8(raw).map_err(|_| ContractError::InvalidInput)
}

fn soroban_u32_vec_to_alloc(values: Vec<u32>) -> alloc::vec::Vec<u32> {
    let mut result = alloc::vec::Vec::new();
    for value in values.iter() {
        result.push(value);
    }
    result
}
