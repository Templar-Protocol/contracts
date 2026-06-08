//! Typed curator and governance operations facade for the Soroban vault.

use alloc::string::String as AllocString;

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, IntoVal,
    InvokeError, String, Symbol, TryFromVal, Val, Vec,
};
use templar_soroban_governance::{
    GovernanceAction, GovernanceActionKind, GovernanceError, PendingProposal, TimelockKind,
    Timelocks,
};
use templar_soroban_shared_types::{
    ProxyPreviewFields, ProxyViewFields, ProxyViewResponse, VaultCommand as WireVaultCommand,
    VaultCommandResult as WireVaultCommandResult,
};

use crate::error::ContractError;

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
    pub max_growth_rate_wad: i128,
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
    pub sentinel: Option<Address>,
    pub timelocks: Timelocks,
    pub pending_ids: Vec<u64>,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub struct Fees {
    pub performance_fee_wad: i128,
    pub performance_recipient: Address,
    pub management_fee_wad: i128,
    pub management_recipient: Address,
    pub max_growth_rate_wad: Option<i128>,
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum Restrictions {
    None,
    Blacklist(Vec<Address>),
    Whitelist(Vec<Address>),
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum CapGroupUpdate {
    SetCap(String, i128),
    SetRelativeCap(String, i128),
    SetMember(u32, String),
}

#[contracttype]
#[derive(Clone, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
pub enum CapGroupUpdateKey {
    Cap(String),
    RelativeCap(String),
    Member(u32),
}

impl Restrictions {
    fn into_parts(self, env: &Env) -> (u32, Vec<Address>) {
        match self {
            Self::None => (0, Vec::new(env)),
            Self::Blacklist(accounts) => (1, accounts),
            Self::Whitelist(accounts) => (2, accounts),
        }
    }
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
        invoke_governance(
            &env,
            "submit_set_fees",
            (
                admin,
                fees.performance_fee_wad,
                fees.performance_recipient,
                fees.management_fee_wad,
                fees.management_recipient,
                fees.max_growth_rate_wad,
            )
                .into_val(&env),
        )
    }

    pub fn set_restrictions(
        env: Env,
        admin: Address,
        restrictions: Restrictions,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        let (mode, accounts) = restrictions.into_parts(&env);
        invoke_governance(
            &env,
            "submit_set_restrictions",
            (admin, mode, accounts).into_val(&env),
        )
    }

    pub fn set_allowed_adapters(
        env: Env,
        admin: Address,
        adapters: Vec<Address>,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_allowed_adapters",
            (admin, adapters).into_val(&env),
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
        kind: TimelockKind,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(
            &env,
            "submit_set_timelock",
            (admin, kind, new_timelock_ns).into_val(&env),
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
        match update {
            CapGroupUpdate::SetCap(group, cap) => invoke_governance(
                &env,
                "submit_set_group_cap",
                (admin, group, cap).into_val(&env),
            ),
            CapGroupUpdate::SetRelativeCap(group, cap) => invoke_governance(
                &env,
                "submit_set_group_rel_cap",
                (admin, group, cap).into_val(&env),
            ),
            CapGroupUpdate::SetMember(market, group) => invoke_governance(
                &env,
                "submit_set_group_member",
                (admin, market, group).into_val(&env),
            ),
        }
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
        invoke_governance(
            &env,
            "submit_set_allocators",
            (admin, allocators).into_val(&env),
        )
    }

    pub fn accept_kind(
        env: Env,
        admin: Address,
        kind: GovernanceActionKind,
    ) -> Result<u64, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "accept_kind", (admin, kind).into_val(&env))
    }

    /// Submit an opaque governance action when no typed proxy method fits.
    ///
    /// Prefer typed submit methods whenever possible. Use a stable, documented
    /// `key` namespace for the action schema, and set `payload_hash` to the
    /// hash of the exact canonical payload bytes the downstream consumer will
    /// verify. Reusing keys across schemas or hashing non-canonical bytes can
    /// create unsafe or unverifiable governance state.
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
        Self::accept_kind(env, admin, GovernanceActionKind::Fees)
    }

    pub fn accept_cap(env: Env, admin: Address, market_id: u32) -> Result<u64, ContractError> {
        accept_pending_selector(env, admin, PendingSelector::Cap(market_id))
    }

    pub fn accept_market_removal(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u64, ContractError> {
        accept_pending_selector(env, admin, PendingSelector::MarketRemoval(market_id))
    }

    pub fn accept_cap_group_update(
        env: Env,
        admin: Address,
        key: CapGroupUpdateKey,
    ) -> Result<u64, ContractError> {
        accept_pending_selector(env, admin, PendingSelector::CapGroup(key))
    }

    pub fn accept_timelock(
        env: Env,
        admin: Address,
        kind: Option<TimelockKind>,
    ) -> Result<u64, ContractError> {
        match kind {
            Some(kind) => accept_pending_selector(env, admin, PendingSelector::Timelock(kind)),
            None => Self::accept_kind(env, admin, GovernanceActionKind::TimelockConfig),
        }
    }

    pub fn accept_allocators(env: Env, admin: Address) -> Result<u64, ContractError> {
        Self::accept_kind(env, admin, GovernanceActionKind::Allocators)
    }

    pub fn revoke(env: Env, admin: Address, proposal_id: u64) -> Result<(), ContractError> {
        admin.require_auth();
        invoke_governance(&env, "revoke", (admin, proposal_id).into_val(&env))
    }

    pub fn revoke_pending_fees(env: Env, admin: Address) -> Result<u32, ContractError> {
        Self::revoke_kind(env, admin, GovernanceActionKind::Fees)
    }

    pub fn revoke_pending_cap(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u32, ContractError> {
        revoke_pending_selector(env, admin, PendingSelector::Cap(market_id))
    }

    pub fn revoke_pending_market_removal(
        env: Env,
        admin: Address,
        market_id: u32,
    ) -> Result<u32, ContractError> {
        revoke_pending_selector(env, admin, PendingSelector::MarketRemoval(market_id))
    }

    pub fn revoke_pending_cap_group_update(
        env: Env,
        admin: Address,
        key: CapGroupUpdateKey,
    ) -> Result<u32, ContractError> {
        revoke_pending_selector(env, admin, PendingSelector::CapGroup(key))
    }

    pub fn revoke_pending_timelock(
        env: Env,
        admin: Address,
        kind: Option<TimelockKind>,
    ) -> Result<u32, ContractError> {
        match kind {
            Some(kind) => revoke_pending_selector(env, admin, PendingSelector::Timelock(kind)),
            None => Self::revoke_kind(env, admin, GovernanceActionKind::TimelockConfig),
        }
    }

    pub fn revoke_pending_allocators(env: Env, admin: Address) -> Result<u32, ContractError> {
        Self::revoke_kind(env, admin, GovernanceActionKind::Allocators)
    }

    pub fn revoke_kind(
        env: Env,
        admin: Address,
        kind: GovernanceActionKind,
    ) -> Result<u32, ContractError> {
        admin.require_auth();
        invoke_governance(&env, "revoke_kind", (admin, kind).into_val(&env))
    }

    /// Revoke pending opaque governance actions by their exact key/hash pair.
    ///
    /// This is only for actions submitted through `submit_other`; prefer typed
    /// revoke helpers for typed proposals. The `key` namespace and
    /// `payload_hash` must match the original canonical payload identifier.
    /// Mismatched or ambiguous values can leave unsafe or unverifiable
    /// governance state pending.
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
        let _ = env;
        Ok(None)
    }

    pub fn sentinel(env: Env) -> Result<Option<Address>, ContractError> {
        invoke_governance(&env, "sentinel", Vec::new(&env))
    }

    pub fn is_abdicated(env: Env, kind: GovernanceActionKind) -> Result<bool, ContractError> {
        invoke_governance(&env, "is_abdicated", (kind,).into_val(&env))
    }

    /// Check whether an opaque key/hash governance action is approved.
    ///
    /// Typed state should use typed query/accept paths instead. For opaque
    /// actions, `key` names the documented action schema and `payload_hash`
    /// must be the hash of the exact canonical payload bytes that will be
    /// consumed. Misuse can make approvals unverifiable or unsafe to execute.
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
        let response = call_proxy_view_full(&env, &owner, assets, shares)?;
        Ok(vault_preview_from_fields(response.preview))
    }

    pub fn governance_view(env: Env) -> Result<GovernanceView, ContractError> {
        Ok(GovernanceView {
            admin: Self::admin(env.clone())?,
            sentinel: Self::sentinel(env.clone())?,
            timelocks: timelocks_from_governance_scalars(&env)?,
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
) -> Result<ProxyViewFields, ContractError> {
    let vault_address = read_vault_address(env)?;
    let result = env.try_invoke_contract::<ProxyViewResponse, ContractError>(
        &vault_address,
        &Symbol::new(env, "proxy_view"),
        (owner.clone(), assets, shares).into_val(env),
    );

    match result {
        Ok(Ok(response)) => Ok(response.into()),
        Ok(Err(_)) => Err(ContractError::VaultError),
        Err(Ok(error)) => Err(error),
        Err(Err(invoke_error)) => Err(match invoke_error {
            InvokeError::Abort => ContractError::VaultError,
            InvokeError::Contract(code) => ContractError::from_vault_error_code(code),
        }),
    }
}

fn vault_view_from_response(response: ProxyViewFields) -> VaultView {
    let core = response.core;
    let policy = response.policy;
    VaultView {
        curator: core.addresses.curator,
        governance: core.addresses.governance,
        asset_token: core.addresses.asset_token,
        share_token: core.addresses.share_token,
        virtual_shares: core.virtual_offsets.virtual_shares,
        virtual_assets: core.virtual_offsets.virtual_assets,
        paused: core.virtual_offsets.paused,
        total_shares: core.totals.total_shares,
        idle_assets: core.totals.idle_assets,
        external_assets: core.totals.external_assets,
        total_assets: core.totals.total_assets,
        fee_anchor_total_assets: core.fees.fee_total_assets,
        fee_anchor_timestamp_ns: core.fees.fee_timestamp_ns,
        management_fee_wad: core.fees.management_fee_wad,
        performance_fee_wad: core.fees.performance_fee_wad,
        max_growth_rate_wad: core.fees.max_total_assets_growth_rate_wad,
        supply_queue: policy.supply_queue,
        cap_groups: policy.cap_groups,
    }
}

fn vault_preview_from_fields(preview: ProxyPreviewFields) -> VaultPreview {
    VaultPreview {
        convert_to_shares: preview.convert_to_shares,
        convert_to_assets: preview.convert_to_assets,
        max_deposit: preview.max_deposit,
        max_mint: preview.max_mint,
        max_withdraw: preview.max_withdraw,
        max_redeem: preview.max_redeem,
        preview_mint_assets: preview.preview_mint_assets,
        preview_withdraw_shares: preview.preview_withdraw_shares,
    }
}

enum PendingSelector {
    Cap(u32),
    MarketRemoval(u32),
    CapGroup(CapGroupUpdateKey),
    Timelock(TimelockKind),
}

impl PendingSelector {
    fn matches(&self, action: &GovernanceAction) -> bool {
        match (self, action) {
            (Self::Cap(expected), GovernanceAction::SetCap(market_id, _)) => expected == market_id,
            (Self::MarketRemoval(expected), GovernanceAction::RemoveMarket(market_id)) => {
                expected == market_id
            }
            (
                Self::CapGroup(CapGroupUpdateKey::Cap(expected)),
                GovernanceAction::SetGroupCap(group, _),
            ) => expected == group,
            (
                Self::CapGroup(CapGroupUpdateKey::RelativeCap(expected)),
                GovernanceAction::SetGroupRelCap(group, _),
            ) => expected == group,
            (
                Self::CapGroup(CapGroupUpdateKey::Member(expected)),
                GovernanceAction::SetGroupMember(market_id, _),
            ) => expected == market_id,
            (Self::Timelock(expected), GovernanceAction::SetTimelock(kind, _)) => expected == kind,
            _ => false,
        }
    }
}

fn accept_pending_selector(
    env: Env,
    admin: Address,
    selector: PendingSelector,
) -> Result<u64, ContractError> {
    let proposal_id = find_single_pending_selector(&env, &selector)?;
    SorobanCuratorProxyContract::accept(env, admin, proposal_id)?;
    Ok(proposal_id)
}

fn revoke_pending_selector(
    env: Env,
    admin: Address,
    selector: PendingSelector,
) -> Result<u32, ContractError> {
    let proposal_id = find_single_pending_selector(&env, &selector)?;
    SorobanCuratorProxyContract::revoke(env, admin, proposal_id)?;
    Ok(1)
}

fn find_single_pending_selector(
    env: &Env,
    selector: &PendingSelector,
) -> Result<u64, ContractError> {
    let pending_ids: Vec<u64> = invoke_governance(env, "pending_ids", Vec::new(env))?;
    let mut matching_id = None;

    for proposal_id in pending_ids.iter() {
        let proposal: PendingProposal =
            invoke_governance(env, "pending", (proposal_id,).into_val(env))?;
        if selector.matches(&proposal.action) && matching_id.replace(proposal.id).is_some() {
            return Err(ContractError::SelectorAmbiguous);
        }
    }

    matching_id.ok_or(ContractError::GovernanceError)
}

fn timelocks_from_governance_scalars(env: &Env) -> Result<Timelocks, ContractError> {
    let pause_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Pause)?;
    let curator_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Curator)?;
    let governance_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Governance)?;
    let supply_queue_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::SupplyQueue)?;
    let fees_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Fees)?;
    let restrictions_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Restrictions)?;
    let sentinel_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Sentinel)?;
    let allocators_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Allocators)?;
    let allowed_adapters_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::AllowedAdapters)?;
    let cap_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Cap)?;
    let market_removal_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::MarketRemoval)?;
    let cap_group_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::CapGroup)?;
    let skim_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Skim)?;
    let upgrade_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Upgrade)?;
    let migration_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Migration)?;
    let timelock_config_ns =
        SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::TimelockConfig)?;
    let other_ns = SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Other)?;

    Ok(Timelocks {
        admin_ns: SorobanCuratorProxyContract::timelock_ns(env.clone(), TimelockKind::Admin)?,
        pause_ns,
        curator_ns,
        governance_ns,
        supply_queue_ns,
        fees_ns,
        restrictions_ns,
        sentinel_ns,
        allocators_ns,
        allowed_adapters_ns,
        cap_ns,
        market_removal_ns,
        cap_group_ns,
        skim_ns,
        upgrade_ns,
        migration_ns,
        timelock_config_ns,
        other_ns,
    })
}

#[cfg(test)]
pub(crate) fn timelocks_from_kind_values(
    mut value_for: impl FnMut(TimelockKind) -> u64,
) -> Timelocks {
    Timelocks {
        admin_ns: value_for(TimelockKind::Admin),
        pause_ns: value_for(TimelockKind::Pause),
        curator_ns: value_for(TimelockKind::Curator),
        governance_ns: value_for(TimelockKind::Governance),
        supply_queue_ns: value_for(TimelockKind::SupplyQueue),
        fees_ns: value_for(TimelockKind::Fees),
        restrictions_ns: value_for(TimelockKind::Restrictions),
        sentinel_ns: value_for(TimelockKind::Sentinel),
        allocators_ns: value_for(TimelockKind::Allocators),
        allowed_adapters_ns: value_for(TimelockKind::AllowedAdapters),
        cap_ns: value_for(TimelockKind::Cap),
        market_removal_ns: value_for(TimelockKind::MarketRemoval),
        cap_group_ns: value_for(TimelockKind::CapGroup),
        skim_ns: value_for(TimelockKind::Skim),
        upgrade_ns: value_for(TimelockKind::Upgrade),
        migration_ns: value_for(TimelockKind::Migration),
        timelock_config_ns: value_for(TimelockKind::TimelockConfig),
        other_ns: value_for(TimelockKind::Other),
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
