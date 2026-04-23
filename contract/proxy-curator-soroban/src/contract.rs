//! Typed curator and governance operations facade for the Soroban vault.

use alloc::string::String as AllocString;

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, IntoVal,
    InvokeError, String, Symbol, TryFromVal, Val, Vec,
};
use templar_soroban_governance::{
    CapGroupUpdate, CapGroupUpdateKey, Fees, GovernanceActionKind, GovernanceError,
    PendingProposal, Restrictions, TimelockKind, Timelocks,
};
use templar_soroban_shared_types::{
    VaultCommand as WireVaultCommand, VaultCommandResult as WireVaultCommandResult,
};

use crate::error::ContractError;

pub(crate) type ProxyCoreView = (
    (Address, Address, Address, Address),
    (i128, i128, bool),
    (i128, i128, i128, i128),
    (i128, u64, i128, i128),
);
pub(crate) type ProxyPolicyView = (Vec<u32>, Vec<(String, i128, i128)>);
pub(crate) type ProxyPreviewView = (i128, i128, i128, i128, i128, i128, i128, i128);
pub(crate) type ProxyViewResponse = (ProxyCoreView, ProxyPolicyView, ProxyPreviewView);

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub enum AllocationDelta {
    Supply(u32, i128),
    Withdraw(u32, i128),
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct VaultView {
    pub curator: Address,
    pub governance: Address,
    pub asset_token: Address,
    pub share_token: Address,
    pub virtual_shares: i128,
    pub virtual_assets: i128,
    pub paused: bool,
    pub total_shares: i128,
    pub idle_assets: i128,
    pub external_assets: i128,
    pub total_assets: i128,
    pub fee_anchor_total_assets: i128,
    pub fee_anchor_timestamp_ns: u64,
    pub management_fee_wad: i128,
    pub performance_fee_wad: i128,
    pub supply_queue: Vec<u32>,
    pub cap_groups: Vec<(String, i128, i128)>,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct VaultPreview {
    pub convert_to_shares: i128,
    pub convert_to_assets: i128,
    pub max_deposit: i128,
    pub max_mint: i128,
    pub max_withdraw: i128,
    pub max_redeem: i128,
    pub preview_mint_assets: i128,
    pub preview_withdraw_shares: i128,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub struct GovernanceView {
    pub admin: Address,
    pub guardian: Option<Address>,
    pub sentinel: Option<Address>,
    pub timelocks: Timelocks,
    pub pending_ids: Vec<u64>,
}

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

    pub fn allocate(
        env: Env,
        allocator: Address,
        delta: AllocationDelta,
    ) -> Result<i128, ContractError> {
        allocator.require_auth();
        let (market, amount, supply) = match delta {
            AllocationDelta::Supply(market, amount) => (market, amount, true),
            AllocationDelta::Withdraw(market, amount) => (market, amount, false),
        };
        expect_i128_result(invoke_vault_execute(
            &env,
            VaultCommand::Allocate {
                caller: allocator,
                market,
                amount,
                supply,
            },
        )?)
    }

    pub fn refresh_markets(
        env: Env,
        operator: Address,
        markets: Vec<u32>,
    ) -> Result<i128, ContractError> {
        operator.require_auth();
        expect_i128_result(invoke_vault_execute(
            &env,
            VaultCommand::RefreshMarkets {
                caller: operator,
                markets,
            },
        )?)
    }

    pub fn resync_idle_balance(env: Env) -> Result<(), ContractError> {
        expect_unit_result(invoke_vault_execute(&env, VaultCommand::ResyncIdleBalance)?)
    }

    pub fn extend_vault_ttl(env: Env) -> Result<(), ContractError> {
        expect_unit_result(invoke_vault_execute(&env, VaultCommand::ExtendTtl)?)
    }

    pub fn cancel_migration(env: Env, admin: Address) -> Result<(), ContractError> {
        admin.require_auth();
        expect_unit_result(invoke_vault_execute(
            &env,
            VaultCommand::CancelMigration { caller: admin },
        )?)
    }

    pub fn set_paused(env: Env, admin: Address, paused: bool) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "submit_set_paused", (admin, paused).into_val(&env))
    }

    pub fn set_curator(
        env: Env,
        admin: Address,
        new_curator: Address,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_curator",
            (admin, new_curator).into_val(&env),
        )
    }

    pub fn set_governance(
        env: Env,
        admin: Address,
        governance: Address,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_governance",
            (admin, governance).into_val(&env),
        )
    }

    pub fn set_supply_queue(
        env: Env,
        admin: Address,
        markets: Vec<u32>,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_supply_queue",
            (admin, markets).into_val(&env),
        )
    }

    pub fn set_fees(env: Env, admin: Address, fees: Fees) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "set_fees", (admin, fees).into_val(&env))
    }

    pub fn set_restrictions(
        env: Env,
        admin: Address,
        restrictions: Restrictions,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "set_restrictions",
            (admin, restrictions).into_val(&env),
        )
    }

    pub fn set_guardian(env: Env, admin: Address, guardian: Address) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_guardian",
            (admin, guardian).into_val(&env),
        )
    }

    pub fn set_sentinel(env: Env, admin: Address, sentinel: Address) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_sentinel",
            (admin, sentinel).into_val(&env),
        )
    }

    pub fn submit_timelock(
        env: Env,
        admin: Address,
        new_timelock_ns: u64,
        kind: Option<TimelockKind>,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_timelock",
            (admin, new_timelock_ns, kind).into_val(&env),
        )
    }

    pub fn submit_cap(
        env: Env,
        admin: Address,
        market_id: u32,
        new_cap: i128,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_cap",
            (admin, market_id, new_cap).into_val(&env),
        )
    }

    pub fn submit_market_removal(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_remove_market",
            (admin, market_id).into_val(&env),
        )
    }

    pub fn submit_cap_group_update(
        env: Env,
        admin: Address,
        update: CapGroupUpdate,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_cap_group_update",
            (admin, update).into_val(&env),
        )
    }

    pub fn set_skim_recipient(
        env: Env,
        admin: Address,
        recipient: Address,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_skim_recipient",
            (admin, recipient).into_val(&env),
        )
    }

    pub fn skim(env: Env, admin: Address, token: Address) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "submit_skim", (admin, token).into_val(&env))
    }

    pub fn set_allocators(
        env: Env,
        admin: Address,
        allocators: Vec<Address>,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "set_allocators", (admin, allocators).into_val(&env))
    }

    pub fn set_is_allocator(
        env: Env,
        admin: Address,
        account: Address,
        allowed: bool,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "set_is_allocator",
            (admin, account, allowed).into_val(&env),
        )
    }

    pub fn submit_other(
        env: Env,
        admin: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_other",
            (admin, key, payload_hash).into_val(&env),
        )
    }

    pub fn accept(env: Env, admin: Address, proposal_id: u64) -> Result<(), ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept", (admin, proposal_id).into_val(&env))
    }

    pub fn accept_fees(env: Env, admin: Address) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept_fees", (admin,).into_val(&env))
    }

    pub fn accept_cap(env: Env, admin: Address, market_id: u32) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept_cap", (admin, market_id).into_val(&env))
    }

    pub fn accept_market_removal(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "accept_market_removal",
            (admin, market_id).into_val(&env),
        )
    }

    pub fn accept_cap_group_update(
        env: Env,
        admin: Address,
        key: CapGroupUpdateKey,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept_cap_group_update", (admin, key).into_val(&env))
    }

    pub fn accept_timelock(
        env: Env,
        admin: Address,
        kind: Option<TimelockKind>,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept_timelock", (admin, kind).into_val(&env))
    }

    pub fn accept_allocators(env: Env, admin: Address) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept_allocators", (admin,).into_val(&env))
    }

    pub fn revoke(env: Env, admin: Address, proposal_id: u64) -> Result<(), ContractError> {
        admin.require_auth();
        invoke_governance(&env, "revoke", (admin, proposal_id).into_val(&env))
    }

    pub fn revoke_pending_fees(env: Env, admin: Address) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "revoke_pending_fees", (admin,).into_val(&env))
    }

    pub fn revoke_pending_cap(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "revoke_pending_cap",
            (admin, market_id).into_val(&env),
        )
    }

    pub fn revoke_pending_market_removal(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "revoke_pending_market_removal",
            (admin, market_id).into_val(&env),
        )
    }

    pub fn revoke_pending_cap_group_update(
        env: Env,
        admin: Address,
        key: CapGroupUpdateKey,
    ) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "revoke_pending_cap_group_update",
            (admin, key).into_val(&env),
        )
    }

    pub fn revoke_pending_timelock(
        env: Env,
        admin: Address,
        kind: Option<TimelockKind>,
    ) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "revoke_pending_timelock",
            (admin, kind).into_val(&env),
        )
    }

    pub fn revoke_pending_allocators(env: Env, admin: Address) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "revoke_pending_allocators", (admin,).into_val(&env))
    }

    pub fn revoke_other_pending(
        env: Env,
        admin: Address,
        key: Symbol,
        payload_hash: BytesN<32>,
    ) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "revoke_other_pending",
            (admin, key, payload_hash).into_val(&env),
        )
    }

    pub fn abdicate(
        env: Env,
        admin: Address,
        kind: GovernanceActionKind,
    ) -> Result<(), ContractError> {
        admin.require_auth();
        invoke_governance(&env, "abdicate", (admin, kind).into_val(&env))
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

    pub fn vault_view(env: Env) -> Result<VaultView, ContractError> {
        let response = call_proxy_view_full(&env, &env.current_contract_address(), 0, 0)?;
        Ok(vault_view_from_response(response))
    }

    pub fn preview(
        env: Env,
        owner: Address,
        assets: i128,
        shares: i128,
    ) -> Result<VaultPreview, ContractError> {
        let (_, _, preview) = call_proxy_view_full(&env, &owner, assets, shares)?;
        Ok(VaultPreview {
            convert_to_shares: preview.0,
            convert_to_assets: preview.1,
            max_deposit: preview.2,
            max_mint: preview.3,
            max_withdraw: preview.4,
            max_redeem: preview.5,
            preview_mint_assets: preview.6,
            preview_withdraw_shares: preview.7,
        })
    }

    pub fn governance_view(env: Env) -> Result<GovernanceView, ContractError> {
        Ok(GovernanceView {
            admin: Self::admin(env.clone())?,
            guardian: Self::guardian(env.clone())?,
            sentinel: Self::sentinel(env.clone())?,
            timelocks: Self::timelocks(env.clone())?,
            pending_ids: Self::pending_ids(env)?,
        })
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

fn call_proxy_view_full(
    env: &Env,
    owner: &Address,
    assets: i128,
    shares: i128,
) -> Result<ProxyViewResponse, ContractError> {
    let vault_address = read_vault_address(env)?;
    let result = env.try_invoke_contract::<ProxyViewResponse, ContractError>(
        &vault_address,
        &Symbol::new(env, "proxy_view"),
        (owner.clone(), assets, shares).into_val(env),
    );

    match result {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err(ContractError::VaultError),
        Err(Ok(error)) => Err(error),
        Err(Err(invoke_error)) => Err(match invoke_error {
            InvokeError::Abort => ContractError::VaultError,
            InvokeError::Contract(code) => ContractError::from_vault_error_code(code),
        }),
    }
}

fn vault_view_from_response(response: ProxyViewResponse) -> VaultView {
    let (core, policy, _) = response;
    let ((curator, governance, asset_token, share_token), virtuals, totals, fees) = core;
    VaultView {
        curator,
        governance,
        asset_token,
        share_token,
        virtual_shares: virtuals.0,
        virtual_assets: virtuals.1,
        paused: virtuals.2,
        total_shares: totals.0,
        idle_assets: totals.1,
        external_assets: totals.2,
        total_assets: totals.3,
        fee_anchor_total_assets: fees.0,
        fee_anchor_timestamp_ns: fees.1,
        management_fee_wad: fees.2,
        performance_fee_wad: fees.3,
        supply_queue: policy.0,
        cap_groups: policy.1,
    }
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
