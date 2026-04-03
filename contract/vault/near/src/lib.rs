#![allow(clippy::needless_pass_by_value)]

use crate::{
    aum::AUM,
    convert::{account_id_to_address, IntoTargetId},
    governance::{Abdicator, Gate, TimelockedAction, Timelocks},
    impl_callbacks::unwrap_or_return,
    kernel_effects::{apply_kernel_effects, KernelEffectContext},
    policy::{MarketExecutionLock, SupplyQueue, WithdrawRoute},
    storage_management::{
        require_attached_at_least, require_attached_for_pending_withdrawal, yocto_for_ft_account,
    },
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, serde_json,
    store::IterableMap,
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue,
};
use near_sdk_contract_tools::{
    ft::{
        nep141::GAS_FOR_FT_TRANSFER_CALL, nep145::Nep145ForceUnregister, ContractMetadata,
        FungibleToken, Nep141Controller, Nep141Mint, Nep145 as _, Nep145Controller,
        Nep148Controller, StorageBalanceBounds,
    },
    Owner, Rbac,
};
use near_sdk_contract_tools::{owner::Owner, rbac};
use near_sdk_contract_tools::{owner::OwnerExternal, rbac::Rbac};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    num::NonZeroU8,
};
use templar_common::{
    asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset},
    market::ext_market,
    panic_with_message,
    vault::{
        prelude::{
            compute_fee_shares, compute_fee_shares_from_assets, MAX_MANAGEMENT_FEE_WAD,
            MAX_PERFORMANCE_FEE_WAD,
        },
        require_at_least, AllocatingState, AllocationDelta, AllocationPlan, CapGroupId,
        CapGroupRecord, Error, Event, FeeAccrualAnchor, Fees, IdleBalanceDelta,
        MarketConfiguration, MarketId, OpState, PayoutState, PendingWithdrawal, QueueAction,
        QueueStatus, RealAssetsReport, Reason, RefreshingState, UnbrickPhase, VaultConfiguration,
        WithdrawProgressPhase, WithdrawingState, AFTER_SEND_TO_USER_GAS, ALLOCATE_GAS,
        CREATE_WITHDRAW_REQ_GAS, EXECUTE_WITHDRAW_GAS, FT_BALANCE_OF_GAS, GET_SUPPLY_POSITION_GAS,
        MAX_TIMELOCK_NS, MIN_TIMELOCK_NS, SUPPLY_AFTER_TRANSFER_CHECK_GAS,
        SUPPLY_POSITION_READ_CALLBACK_GAS, WITHDRAW_CREATE_REQUEST_CALLBACK_GAS, YEAR_NS,
    },
};
pub use templar_curator_primitives::rbac::Role;
use templar_curator_primitives::{
    determine_recovery_action, PendingValue, RecoveryContext, RecoveryProgress,
};
use templar_vault_kernel::actions::apply_action;
use templar_vault_kernel::state::op_state::AllocationPlanEntry;
use templar_vault_kernel::state::queue::{
    compute_idle_settlement, is_past_cooldown, DEFAULT_COOLDOWN_NS,
};
use templar_vault_kernel::{Address, KernelAction, PayoutOutcome, TimestampNs};

const DEFAULT_REFRESH_COOLDOWN_NS: u64 = 30_000_000_000; // 30 seconds
const DEFAULT_IDLE_RESYNC_COOLDOWN_NS: u64 = 120_000_000_000;
const ERR_WITHDRAW_DURING_IDLE_RESYNC: &str = "Cannot withdraw/redeem during idle resync";
const ERR_MISSING_WITHDRAWAL_QUEUE_ADDRESS: &str = "Missing address mapping for withdrawal queue";

pub use templar_common::vault::prelude::{mul_div_ceil, mul_div_floor, Number, Wad};

pub mod aum;
pub(crate) mod convert;
pub mod governance;
pub(crate) mod kernel_effects;
pub(crate) mod kernel_mirror;
pub mod policy;

pub(crate) mod auth;
pub mod impl_callbacks;
pub mod impl_token_receiver;
pub(crate) mod op_guard;
pub mod storage_management;

mod impl_vault_external;

#[cfg(test)]
mod test_utils;

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(
    Clone, Copy, PartialEq, Eq, near_sdk::BorshStorageKey, near_sdk::borsh::BorshSerialize,
)]
pub enum StorageKey {
    PendingWithdrawals,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct MarketRecord {
    pub account: AccountId,
    pub cfg: MarketConfiguration,
    pub principal: u128,
}

impl MarketRecord {
    pub fn new(account: AccountId) -> Self {
        Self {
            account,
            cfg: MarketConfiguration::default(),
            principal: 0,
        }
    }

    pub fn with_parts(account: AccountId, cfg: MarketConfiguration, principal: u128) -> Self {
        Self {
            account,
            cfg,
            principal,
        }
    }
}

#[near(serializers = [borsh])]
/// Legacy contract state (pre-kernel withdraw queue).
///
/// Used only for state migration to the kernel-backed `WithdrawQueue`.
struct OldContract {
    underlying_asset: FungibleAsset<BorrowAsset>,
    aum: AUM,
    fees: Fees<Wad>,
    skim_recipient: AccountId,
    fee_anchor: FeeAccrualAnchor,
    idle_balance: u128,
    op_state: OpState,
    next_op_id: u64,
    last_refresh_ns: u64,
    refresh_cooldown_ns: u64,
    idle_resync_last_ns: u64,
    idle_resync_cooldown_ns: u64,
    idle_resync_inflight_op_id: u64,
    virtual_shares: u128,
    virtual_assets: u128,
    markets: BTreeMap<MarketId, MarketRecord>,
    market_ids: BTreeMap<AccountId, MarketId>,
    cap_groups: BTreeMap<CapGroupId, CapGroupRecord>,
    next_market_id: u32,
    governance_timelocks: Timelocks,
    supply_queue: Vec<MarketId>,
    pending_withdrawals: IterableMap<u64, PendingWithdrawal>,
    next_withdraw_to_execute: u64,
    market_execution_lock: templar_common::vault::Locker,
    withdraw_route: Vec<MarketId>,
    abdicator: Abdicator,
    gate: Gate,
}

#[derive(PanicOnDefault, FungibleToken, Owner, Rbac)]
#[fungible_token(force_unregister_hook = "Self")]
#[rbac(roles = "Role", crate = "crate")]
#[near(contract_state)]
/// Vault contract issuing NEP-141 shares over a BorrowAsset.
/// Uses 4626-like deposit/withdraw flows with queued withdrawals and allocator-routed markets.
///
/// Critical invariants
/// - Assets accounting is correct: total_assets = idle_balance + sum(all principals in markets).
/// - Only one op in flight (op_state); mutating ops require Idle.
/// - Governance changes obey timelocks; Sentinel may revoke pending changes.
///
/// Note: RBAC storage is paid by the contract; callers are not charged deposits for RBAC changes.
pub struct Contract {
    /// The underlying asset that the vault manages
    underlying_asset: FungibleAsset<BorrowAsset>,
    /// The process in which the vault calculates its assets under management
    aum: AUM,
    /// Fees (rate + recipient)
    fees: Fees<Wad>,
    /// The recipient of any skimmed tokens that are erroneously held by the vault
    skim_recipient: AccountId,
    /// Fee accrual anchor (assets + timestamp)
    fee_anchor: FeeAccrualAnchor,
    /// Vaults liquidity buffer
    idle_balance: u128,

    /// The vault's operation state
    op_state: OpState,
    /// The next operation id
    next_op_id: u64,
    /// Last timestamp a refresh_markets call succeeded
    last_refresh_ns: u64,
    /// Cooldown between refresh_markets calls (ns)
    refresh_cooldown_ns: u64,
    /// Cooldown before a withdrawal can be executed (ns)
    withdrawal_cooldown_ns: u64,

    idle_resync_last_ns: u64,
    idle_resync_cooldown_ns: u64,
    idle_resync_inflight_op_id: u64,

    /// Virtual offsets used only in conversions/previews to harden edge cases
    virtual_shares: u128,
    virtual_assets: u128,

    /// Markets controlled by the vault, keyed by stable MarketId.
    markets: BTreeMap<MarketId, MarketRecord>,
    /// Reverse lookup from market AccountId to MarketId.
    market_ids: BTreeMap<AccountId, MarketId>,
    /// Cap groups for correlated risk throttling
    cap_groups: BTreeMap<CapGroupId, CapGroupRecord>,

    /// Next identifier to assign when creating a market
    next_market_id: u32,

    /// Per‑action governance timelock configuration.
    governance_timelocks: Timelocks,

    /// Ordered list of market IDs for deposit allocation
    supply_queue: SupplyQueue,

    /// Pending withdrawals queue (kernel canonical storage)
    withdraw_queue: templar_vault_kernel::WithdrawQueue,
    /// Reverse lookup for kernel Address -> AccountId (queue actors)
    address_book: BTreeMap<Address, AccountId>,

    // indices of markets with created requests (per withdrawing op)
    market_execution_lock: MarketExecutionLock,

    // Keeper-provided withdraw route for the current Withdrawing op
    withdraw_route: WithdrawRoute,

    abdicator: Abdicator,
    gate: Gate,
}

#[near]
impl Contract {
    #[allow(clippy::unwrap_used, reason = "Infallible")]
    #[init]
    #[must_use]
    pub fn new(configuration: VaultConfiguration) -> Self {
        let VaultConfiguration {
            owner,
            curator,
            sentinel,
            underlying_token,
            initial_timelock_ns,
            skim_recipient,
            name,
            symbol,
            decimals,
            restrictions,
            fees,
            refresh_cooldown_ns,
            idle_resync_cooldown_ns,
            withdrawal_cooldown_ns,
        } = configuration;

        require!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&initial_timelock_ns.0),
            "timelock bounds"
        );

        require!(
            fees.management.fee <= Wad::from(MAX_MANAGEMENT_FEE_WAD),
            "management fee too high"
        );
        require!(
            fees.performance.fee <= Wad::from(MAX_PERFORMANCE_FEE_WAD),
            "performance fee too high"
        );

        let mut contract = Self {
            underlying_asset: underlying_token,
            aum: AUM::BalanceSheet,
            fees,
            skim_recipient,
            fee_anchor: FeeAccrualAnchor {
                total_assets: U128::default(),
                timestamp_ns: env::block_timestamp().into(),
            },
            markets: BTreeMap::new(),
            market_ids: BTreeMap::new(),
            cap_groups: BTreeMap::new(),
            governance_timelocks: TimestampNs(initial_timelock_ns.0).into(),
            next_market_id: 0,
            supply_queue: SupplyQueue::default(),
            virtual_shares: 1,
            virtual_assets: 1,
            idle_balance: 0,
            op_state: OpState::Idle,
            next_op_id: 1,
            last_refresh_ns: 0,
            refresh_cooldown_ns: refresh_cooldown_ns.map_or(DEFAULT_REFRESH_COOLDOWN_NS, |v| v.0),
            withdrawal_cooldown_ns: withdrawal_cooldown_ns.map_or(DEFAULT_COOLDOWN_NS, |v| v.0),
            idle_resync_last_ns: 0,
            idle_resync_cooldown_ns: idle_resync_cooldown_ns
                .map_or(DEFAULT_IDLE_RESYNC_COOLDOWN_NS, |v| v.0),
            idle_resync_inflight_op_id: 0,
            withdraw_queue: templar_vault_kernel::WithdrawQueue::new(),
            address_book: BTreeMap::new(),
            market_execution_lock: MarketExecutionLock::default(),
            withdraw_route: WithdrawRoute::default(),
            abdicator: Abdicator::new(),
            gate: Gate::new(restrictions),
        };

        contract.set_metadata(&ContractMetadata::new(name, symbol, decimals.into()));
        Owner::init(&mut contract, &owner);
        Rbac::add_role(&mut contract, &curator, &Role::Curator);
        Rbac::add_role(&mut contract, &curator, &Role::Allocator);
        Rbac::add_role(&mut contract, &sentinel, &Role::Sentinel);

        contract.set_storage_balance_bounds(&StorageBalanceBounds {
            min: NearToken::from_yoctonear(yocto_for_ft_account()),
            max: None,
        });

        contract
    }

    #[init(ignore_state)]
    #[must_use]
    pub fn migrate() -> Self {
        let Some(old) = env::state_read() else {
            panic_with_message("No contract state to migrate");
        };
        let old: OldContract = old;
        old.into_current()
    }

    /// Burns the necessary shares to withdraw `amount` of underlying to `receiver`.
    /// Internally calls `redeem` after computing the share amount.
    #[payable]
    pub fn withdraw(&mut self, amount: U128, receiver: AccountId) -> PromiseOrValue<()> {
        require_at_least(templar_common::vault::WITHDRAW_GAS);
        if self.idle_resync_inflight_op_id != 0 {
            panic_with_message(ERR_WITHDRAW_DURING_IDLE_RESYNC);
        }
        self.internal_accrue_fee();
        let shares_needed = self.preview_withdraw(amount).0;
        Event::WithdrawPreview {
            shares: U128(shares_needed),
            receiver: receiver.clone(),
        }
        .emit();
        self.redeem(U128(shares_needed), receiver)
    }

    /// Redeems `shares` for underlying assets sent to `receiver`.
    /// Shares are escrowed to the contract and only burned after successful payout.
    #[payable]
    pub fn redeem(&mut self, shares: U128, receiver: AccountId) -> PromiseOrValue<()> {
        let shares = shares.0;
        let sender = env::predecessor_account_id();

        self.gate.enforce_policy(&sender);
        self.gate.enforce_policy(&receiver);

        require!(shares > 0, "Invalid shares");

        if self.idle_resync_inflight_op_id != 0 {
            panic_with_message(ERR_WITHDRAW_DURING_IDLE_RESYNC);
        }

        let _ = require_attached_for_pending_withdrawal();

        self.internal_accrue_fee();

        let now = env::block_timestamp();
        let expected_assets = self.convert_to_assets(U128(shares)).0;
        require!(expected_assets > 0, "Dust redeem would yield 0 assets");

        let kernel_state = self.kernel_state_mirror();
        let kernel_config = self.kernel_config_mirror();
        let kernel_restrictions = self.kernel_restrictions_mirror();
        let owner_addr = account_id_to_address(&sender);
        let receiver_addr = account_id_to_address(&receiver);
        let self_addr = account_id_to_address(&env::current_account_id());
        let request_id = self.withdraw_queue.next_pending_withdrawal_id;

        let result = apply_action(
            kernel_state,
            &kernel_config,
            kernel_restrictions.as_ref(),
            &self_addr,
            KernelAction::RequestWithdraw {
                owner: owner_addr,
                receiver: receiver_addr,
                shares,
                min_assets_out: expected_assets,
                now_ns: TimestampNs(now),
            },
        )
        .unwrap_or_else(|_| panic_with_message("Kernel request_withdraw failed"));

        self.remember_account_mapping(owner_addr, sender.clone());
        self.remember_account_mapping(receiver_addr, receiver.clone());

        let mut ctx = KernelEffectContext::default();
        ctx.insert(owner_addr, sender.clone());
        ctx.insert(receiver_addr, receiver.clone());
        ctx.insert(self_addr, env::current_account_id());

        apply_kernel_effects(self, &result.effects, &ctx)
            .unwrap_or_else(|_| panic_with_message("Failed to apply kernel withdraw effects"));

        self.withdraw_queue = result.state.withdraw_queue;
        self.rebuild_live_address_book();

        Event::RedeemRequested {
            shares: U128(shares),
            estimated_assets: U128(expected_assets),
        }
        .emit();

        Event::WithdrawalQueued {
            id: request_id.into(),
            owner: sender,
            receiver,
            escrow_shares: U128(shares),
            expected_assets: U128(expected_assets),
            requested_at: now.into(),
        }
        .emit();
        PromiseOrValue::Value(())
    }

    /// Executes the withdraw route provided by the allocator.
    /// If `route` is empty, try to settle with the idle balance.
    pub fn execute_withdrawal(&mut self, route: Vec<MarketId>) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        self.ensure_idle();
        crate::auth::require_action(crate::auth::ActionKind::ExecuteWithdraw);

        self.internal_accrue_fee();

        while let Some((id, pending)) = self.withdraw_queue.head() {
            let now = env::block_timestamp();
            if !is_past_cooldown(
                pending.requested_at_ns,
                TimestampNs(now),
                self.withdrawal_cooldown_ns,
            ) {
                return PromiseOrValue::Value(());
            }

            let owner = self.resolve_account(&pending.owner);
            let receiver = self.resolve_account(&pending.receiver);

            Event::WithdrawProgress {
                phase: WithdrawProgressPhase::ExecutionStarted,
                op_id: None,
                id: Some(id.into()),
                market: None,
                owner: Some(owner.clone()),
                receiver: Some(receiver.clone()),
                escrow_shares: Some(U128(pending.escrow_shares)),
                expected_assets: Some(U128(pending.expected_assets)),
                requested_at: Some(u64::from(pending.requested_at_ns).into()),
            }
            .emit();

            let expected_assets = pending.expected_assets;

            // Skip dust request to avoid wedging the queue.
            if expected_assets == 0 {
                Event::WithdrawProgress {
                    phase: WithdrawProgressPhase::SkippedDust,
                    op_id: None,
                    id: Some(id.into()),
                    market: None,
                    owner: None,
                    receiver: None,
                    escrow_shares: None,
                    expected_assets: None,
                    requested_at: None,
                }
                .emit();
                self.pop_head();
                continue;
            }

            return self.start_withdraw(
                pending.expected_assets,
                &receiver,
                &owner,
                pending.escrow_shares,
                route,
            );
        }

        PromiseOrValue::Value(())
    }

    /// Allocator-only. Progress the current Withdrawing op by executing `market`
    /// from the `withdraw_route`.
    /// Use when offchain signals the vault is next in the market queue.
    pub fn execute_market_withdrawal(
        &mut self,
        op_id: U64,
        market: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        crate::auth::require_action(crate::auth::ActionKind::ExecuteWithdraw);

        let ctx = match self.ctx_withdrawing(op_id.0) {
            Ok(s) => s.clone(),
            Err(_) => panic_with_message("Not withdrawing"),
        };

        let Some(route_index) = self
            .withdraw_route
            .iter()
            .position(|m| *m == market)
            .and_then(|idx| u32::try_from(idx).ok())
        else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market)));
        };

        if ctx.index != route_index {
            self.set_op_state(OpState::Withdrawing(WithdrawingState {
                index: route_index,
                ..ctx
            }));
        }

        self.market_execution_lock.lock(
            market,
            op_id.0,
            u64::MAX.saturating_sub(env::block_timestamp()),
        );

        PromiseOrValue::Promise(
            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                .with_static_gas(FT_BALANCE_OF_GAS)
                .with_unused_gas_weight(0)
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_unused_gas_weight(100)
                        .execute_withdraw_01_execute_withdraw_fetch_position(
                            op_id.into(),
                            market,
                            batch_limit,
                        ),
                ),
        )
    }

    /// Allocator/Curator/Sentinel/Owner only. Executes an existing market-side supply withdrawal
    /// request for `market` and credits any returned underlying to the vault's `idle_balance`,
    /// without touching the user withdrawal queue (`withdraw_queue`) or the payout state
    /// machine.
    ///
    /// This is intended as a pure rebalance operation:
    /// - (Aside from fee accrual) `total_assets` and `total_supply` are preserved.
    /// - Only per-market principal and `idle_balance` are updated by the async callbacks.
    /// - No pending user withdrawal is dequeued or paid out.
    ///
    /// Implementation details:
    /// - Uses `OpState::Allocating` as a generic in-flight guard for this rebalance op.
    /// - Locks the target market index in `market_execution_lock` to serialize the underlying
    ///   market call.
    ///
    /// Expects that a supply withdrawal request for this vault already exists in the given
    /// `market` and is ready to be executed.
    pub fn execute_rebalance_withdrawal(
        &mut self,
        market_id: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        crate::auth::require_action(crate::auth::ActionKind::AbortWithdrawing);

        self.ensure_idle();

        let batch_limit = batch_limit.filter(|n| *n > 0);

        let op_id = self.next_op_id;

        if self.market_record_by_id(market_id).is_none() {
            Event::RebalanceWithdrawStopped {
                op_id: op_id.into(),
                market: market_id,
                reason: Some(Reason::Other("Unknown market".to_string())),
            }
            .emit();
            return PromiseOrValue::Value(());
        }

        self.internal_accrue_fee();

        let principal = self.principal_of(market_id);
        require!(principal > 0, "No principal to withdraw");

        self.market_execution_lock.lock(
            market_id,
            op_id,
            u64::MAX.saturating_sub(env::block_timestamp()),
        );

        // Use Allocating as a generic in-flight guard for this rebalancing op.
        self.next_op_id = op_id.saturating_add(1);
        self.set_op_state(OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: 0,
            plan: Vec::new(),
        }));

        PromiseOrValue::Promise(
            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                .with_static_gas(FT_BALANCE_OF_GAS)
                .with_unused_gas_weight(0)
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_unused_gas_weight(100)
                        .rebalance_withdraw_01_execute_withdraw_fetch_position(
                            op_id,
                            market_id,
                            batch_limit,
                            U128(principal),
                        ),
                ),
        )
    }

    /// Permissionless and throttled.
    /// Refresh principals from markets and return a live assets report; updates stored principals.
    /// Pass an empty `markets` vector to refresh all configured markets.
    pub fn refresh_markets(&mut self, markets: Vec<MarketId>) -> PromiseOrValue<RealAssetsReport> {
        let mut idle = crate::op_guard::IdleGuard::new(self);

        let mut plan = idle.refresh_targets(markets);
        plan.sort_unstable();
        plan.dedup();

        let now = env::block_timestamp();
        let (refresh_plan, refresh_throttle) = {
            let targets: Vec<u32> = plan.iter().map(IntoTargetId::into_target_id).collect();
            templar_curator_primitives::policy::target_set::build_refresh_plan_from_targets(
                &targets,
                idle.refresh_cooldown_ns,
                (idle.last_refresh_ns != 0).then_some(idle.last_refresh_ns),
            )
            .unwrap_or_else(|_| panic_with_message("Invalid refresh plan"))
        };
        let refresh_throttle = refresh_throttle
            .try_acquire(now)
            .unwrap_or_else(|_| panic_with_message("Refresh throttled"));

        let op_id = idle.next_op_id;
        idle.next_op_id = idle.next_op_id.saturating_add(1);

        Event::RefreshStarted {
            op_id: op_id.into(),
            markets: plan.clone(),
            caller: env::predecessor_account_id(),
        }
        .emit();

        let kernel_plan = refresh_plan.into_targets();
        let kernel_state = idle.kernel_state_mirror();
        let kernel_config = idle.kernel_config_mirror();
        let kernel_restrictions = idle.kernel_restrictions_mirror();
        let self_addr = account_id_to_address(&env::current_account_id());

        let result = apply_action(
            kernel_state,
            &kernel_config,
            kernel_restrictions.as_ref(),
            &self_addr,
            KernelAction::BeginRefreshing {
                op_id,
                plan: kernel_plan,
                now_ns: TimestampNs(now),
            },
        )
        .unwrap_or_else(|_| panic_with_message("Kernel begin refresh failed"));

        idle.apply_kernel_op_state(&result.state.op_state);
        idle.next_op_id = result.state.next_op_id;

        idle.refresh_step(op_id)
    }

    /// Permissionless and throttled.
    /// Re-syncs idle_balance to the vault's actual underlying FT balance.
    /// Blocking: sets op_state to Allocating during the async balance read.
    pub fn resync_idle_balance(
        &mut self,
    ) -> PromiseOrValue<templar_common::vault::ResyncIdleReport> {
        require_at_least(templar_common::vault::RESYNC_IDLE_GAS);
        self.ensure_idle();

        if self.idle_resync_inflight_op_id != 0 {
            panic_with_message("Idle resync already in flight")
        }

        let now = env::block_timestamp();
        require!(
            now.saturating_sub(self.idle_resync_last_ns) >= self.idle_resync_cooldown_ns,
            "Idle resync throttled"
        );

        self.idle_resync_last_ns = now;

        self.internal_accrue_fee();

        let op_id = self.next_op_id;
        self.next_op_id = self.next_op_id.saturating_add(1);

        self.idle_resync_inflight_op_id = op_id;

        let before_idle = self.idle_balance;
        let caller = env::predecessor_account_id();

        self.set_op_state(OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: 0,
            plan: Vec::new(),
        }));

        Event::IdleResyncStarted {
            op_id: op_id.into(),
            caller: caller.clone(),
            before_idle: U128(before_idle),
            started_at_ns: now.into(),
        }
        .emit();

        PromiseOrValue::Promise(
            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                .with_static_gas(FT_BALANCE_OF_GAS)
                .with_unused_gas_weight(0)
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(templar_common::vault::RESYNC_IDLE_CALLBACK_GAS)
                        .resync_idle_balance_01_settle(
                            op_id,
                            caller.clone(),
                            U128(before_idle),
                            now,
                        ),
                ),
        )
    }

    /// Allocator/Curator/Owner only. Unbricks the current in-flight withdrawal or payout:
    /// - If Withdrawing: refunds escrowed shares to the owner and dequeues the pending request.
    /// - If in Payout: re-syncs idle_balance with the underlying FT balance,
    ///   refunds escrowed shares to the owner, and dequeues the pending request.
    /// - Clears withdraw state and market execution locks and returns the vault to Idle.
    pub fn unbrick(&mut self) -> PromiseOrValue<()> {
        crate::auth::require_action(crate::auth::ActionKind::AbortWithdrawing);

        let kernel_state = self.op_state.clone();
        let now = env::block_timestamp();
        let context = RecoveryContext::forced(now);
        let progress = match &kernel_state {
            OpState::Allocating(state) => RecoveryProgress::new(state.op_id, now),
            OpState::Withdrawing(state) => RecoveryProgress::new(state.op_id, now),
            OpState::Refreshing(state) => RecoveryProgress::new(state.op_id, now),
            OpState::Payout(_) => return PromiseOrValue::Value(()),
            OpState::Idle => return PromiseOrValue::Value(()),
        };
        let Some(action) = determine_recovery_action(&kernel_state, &context, &progress, None)
            .unwrap_or_else(|_| None)
        else {
            return PromiseOrValue::Value(());
        };

        self.apply_kernel_recovery_action(action)
    }

    fn apply_kernel_recovery_action(&mut self, action: KernelAction) -> PromiseOrValue<()> {
        match action {
            KernelAction::AbortAllocating { op_id, .. } => {
                if matches!(self.op_state, OpState::Allocating(ref s) if s.op_id == op_id) {
                    self.stop_and_exit_allocating::<&str>(None);
                }
                PromiseOrValue::Value(())
            }
            KernelAction::AbortWithdrawing { op_id, .. } => {
                if matches!(self.op_state, OpState::Withdrawing(ref s) if s.op_id == op_id) {
                    let id = self.withdraw_queue.next_withdraw_to_execute;
                    Event::UnbrickInvoked {
                        phase: UnbrickPhase::Withdrawing,
                        op_id: Some(op_id.into()),
                        id: Some(id.into()),
                    }
                    .emit();

                    self.stop_and_exit_withdrawing::<&str>(None);
                }
                PromiseOrValue::Value(())
            }
            KernelAction::AbortRefreshing { op_id } => {
                if matches!(self.op_state, OpState::Refreshing(ref s) if s.op_id == op_id) {
                    let _ = self.stop_and_exit::<&str>(Some(&"abort_refreshing"));
                }
                PromiseOrValue::Value(())
            }
            KernelAction::SettlePayout { op_id, outcome } => {
                if !matches!(self.op_state, OpState::Payout(ref s) if s.op_id == op_id) {
                    return PromiseOrValue::Value(());
                }

                let id = self.withdraw_queue.next_withdraw_to_execute;
                Event::UnbrickInvoked {
                    phase: UnbrickPhase::Payout,
                    op_id: Some(op_id.into()),
                    id: Some(id.into()),
                }
                .emit();

                match outcome {
                    PayoutOutcome::Success => {
                        let (receiver, amount) = match &self.op_state {
                            OpState::Payout(s) => (s.receiver, s.amount),
                            _ => return PromiseOrValue::Value(()),
                        };
                        self.payment_01_reconcile_idle_or_refund(
                            Ok(()),
                            op_id,
                            self.resolve_account(&receiver),
                            U128(amount),
                        );
                        PromiseOrValue::Value(())
                    }
                    PayoutOutcome::Failure => {
                        // Treat stuck payout as failure, but re-sync idle_balance using
                        // the actual underlying FT balance held by the vault account.
                        PromiseOrValue::Promise(
                            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                                .with_static_gas(FT_BALANCE_OF_GAS)
                                .ft_balance_of(env::current_account_id())
                                .then(
                                    Self::ext(env::current_account_id())
                                        .with_static_gas(AFTER_SEND_TO_USER_GAS)
                                        .stop_and_exit_payout_01_reconcile(
                                            op_id,
                                            Some(Reason::Other("unbrick_payout".to_string())),
                                        ),
                                ),
                        )
                    }
                }
            }
            _ => PromiseOrValue::Value(()),
        }
    }

    /// Sends the entire balance of `token` held by the vault to the `skim_recipient`.
    pub fn skim(&mut self, token: AccountId) -> Promise {
        Self::require_owner();

        // Disallow skimming underlying or this own share token
        let share_token_id = env::current_account_id();
        let underlying_token_id = self.underlying_asset.contract_id();

        require!(token != share_token_id, "Refusing to skim the share token");
        require!(
            token != underlying_token_id,
            "Refusing to skim the underlying token"
        );

        self.ensure_idle();

        ext_ft_core::ext(token.clone())
            .with_static_gas(Gas::from_tgas(3))
            .ft_balance_of(env::current_account_id())
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(Gas::from_tgas(10))
                    .skim_01_read_balance(token, self.skim_recipient.clone()),
            )
    }

    /// Allocates assets across markets according to the provided weights.
    /// If `amount` is provided, it is used as the target amount for each market.
    /// Otherwise, the vault will attempt to allocate as much as possible.
    ///
    /// NOTE: Each allocation takes roughly [`ALLOCATE_GAS`] gas. (~21 TGAS)
    pub fn allocate(&mut self, delta: AllocationDelta) -> PromiseOrValue<()> {
        match &delta {
            AllocationDelta::Supply(_) => {
                crate::auth::require_action(crate::auth::ActionKind::BeginAllocating);
            }
            AllocationDelta::Withdraw(_) => {
                crate::auth::require_action(crate::auth::ActionKind::AbortWithdrawing);
            }
        }
        self.ensure_idle();
        self.internal_accrue_fee();
        delta.as_ref().validate();

        match delta {
            AllocationDelta::Supply(delta) => {
                require_at_least(ALLOCATE_GAS);
                let total = self.clamp_allocation_total(Some(delta.amount.0));
                let plan = vec![(delta.market, total)];

                Event::AllocationPlanSet {
                    op_id: self.next_op_id.into(),
                    total: U128(total),
                    plan: plan
                        .iter()
                        .copied()
                        .map(|(market, amount)| (market, amount.into()))
                        .collect(),
                }
                .emit();

                self.start_allocation(total, plan)
            }
            AllocationDelta::Withdraw(delta) => {
                require_at_least(WITHDRAW_CREATE_REQUEST_CALLBACK_GAS);

                let to_request = self.principal_of(delta.market).min(delta.amount.0);
                require!(to_request > 0, "Insufficient principal");

                let market_id = delta.market;

                Event::SupplyWithdrawRequestCreated {
                    market: market_id,
                    amount: U128(to_request),
                }
                .emit();

                let amount = U128(to_request);

                let market_account = self.market_account_by_id_or_panic(market_id).clone();

                PromiseOrValue::Promise(
                    ext_market::ext(market_account)
                        .with_static_gas(CREATE_WITHDRAW_REQ_GAS)
                        .create_supply_withdrawal_request(BorrowAssetAmount::from(amount))
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(WITHDRAW_CREATE_REQUEST_CALLBACK_GAS)
                                .rebalance_withdraw_01_after_create_request(market_id, amount),
                        ),
                )
            }
        }
    }
}

/* ----- Views ----- */
#[near]
impl Contract {
    /// # Panics
    /// - If the owner is not set
    /// - If the curator is not set
    #[allow(clippy::expect_used, reason = "No side effects")]
    pub fn get_configuration(&self) -> VaultConfiguration {
        let meta = self.get_metadata();
        let role_member = |role: &Role, name: &'static str| {
            Self::with_members_of(role, |members| {
                require!(
                    members.len() == 1,
                    format!("Invariant violation: Cannot have more than one {name}")
                );
                members.iter().next().unwrap_or_else(|| {
                    panic_with_message(&format!("{name} not set in get_configuration"))
                })
            })
        };

        VaultConfiguration {
            owner: self.own_get_owner().unwrap_or_else(|| {
                templar_common::panic_with_message("Owner not set in get_configuration")
            }),
            curator: role_member(&Role::Curator, "Curator"),
            sentinel: role_member(&Role::Sentinel, "Sentinel"),
            underlying_token: self.underlying_asset.clone(),
            initial_timelock_ns: u64::from(self.governance_timelocks.timelock_config_ns).into(),
            fees: self.fees.clone(),
            skim_recipient: self.skim_recipient.clone(),
            name: meta.name,
            symbol: meta.symbol,
            decimals: NonZeroU8::new(meta.decimals).expect("Decimals must be non-zero"),
            restrictions: self.gate.restrictions.clone(),
            refresh_cooldown_ns: Some(self.refresh_cooldown_ns.into()),
            idle_resync_cooldown_ns: Some(self.idle_resync_cooldown_ns.into()),
            withdrawal_cooldown_ns: Some(self.withdrawal_cooldown_ns.into()),
        }
    }

    /// Returns all pending timelocked governance actions.
    pub fn get_pending_governance_actions(&self) -> Vec<PendingValue<TimelockedAction>> {
        self.governance_timelocks.pending_actions()
    }

    /// Returns total assets under management = idle balance + sum of market principals.
    pub fn get_total_assets(&self) -> U128 {
        self.aum.get_total_assets(self)
    }

    pub fn get_last_total_assets(&self) -> U128 {
        self.fee_anchor.total_assets
    }

    pub fn get_idle_balance(&self) -> U128 {
        self.idle_balance.into()
    }

    pub fn get_total_supply(&self) -> U128 {
        U128(self.total_supply())
    }

    pub fn get_cap_groups(&self) -> Vec<(CapGroupId, CapGroupRecord)> {
        self.cap_groups
            .iter()
            .map(|(id, rec)| (id.clone(), rec.clone()))
            .collect()
    }

    pub fn get_fee_anchor(&self) -> FeeAccrualAnchor {
        self.fee_anchor.clone()
    }

    pub fn get_fees(&self) -> Fees<U128> {
        self.fees.clone().into()
    }

    /// Returns a best-effort estimate of the maximum additional amount that can be deposited
    /// across all markets given current caps (including cap-group relative-to-AUM caps)
    /// and the current `supply_queue`.
    ///
    /// For relative caps, the bound depends on total assets, so this is computed as a
    /// fixed point: `x <= max_allocatable_room(total_assets + x)`.
    ///
    /// This does not reserve capacity and may become stale immediately after it is read.
    pub fn get_max_deposit(&self) -> U128 {
        let base_total_assets = self.total_assets_for_caps();
        let rounding_slack = self.relative_cap_rounding_slack();

        let markets = self.supply_queue_market_infos();

        let mut low = 0u128;
        let mut high = markets
            .iter()
            .fold(0u128, |acc, m| acc.saturating_add(m.cap_room));

        while low < high {
            let diff = high.saturating_sub(low);
            let mid = low.saturating_add(diff.saturating_add(1) / 2);

            let total_assets = base_total_assets.saturating_add(mid);
            let room = self.max_allocatable_room_at_precomputed(total_assets, &markets);

            if mid <= room.saturating_add(rounding_slack) {
                low = mid;
            } else {
                high = mid.saturating_sub(1);
            }
        }

        U128(low)
    }

    /// Returns a best-effort estimate of the maximum additional amount that can be deposited
    /// into any single market in the current `supply_queue`, given current caps.
    ///
    /// This is intended for UIs that want to route deposits in a way that is consistent with
    /// the vault's `supply_queue`. It does not reserve capacity and may become stale
    /// immediately after it is read.
    pub fn get_max_single_market_deposit(&self) -> U128 {
        let base_total_assets = self.total_assets_for_caps();
        let max_room = self.supply_queue.iter().fold(0u128, |acc, market_id| {
            let Some(rec) = self.market_record_by_id(*market_id) else {
                return acc;
            };

            let cap_room = rec.cfg.cap.0.saturating_sub(rec.principal);
            if cap_room == 0 {
                return acc;
            }

            let room = if let Some(cap_group) = rec.cfg.cap_group_id.as_ref() {
                self.max_single_market_room_in_group(cap_group, cap_room, base_total_assets)
            } else {
                cap_room
            };

            acc.max(room)
        });
        U128(max_room)
    }

    /// Converts an amount of underlying assets to shares, flooring the result.
    /// Uses virtual offsets and fee-aware totals (pre-accrual simulation).
    pub fn convert_to_shares(&self, assets: U128) -> U128 {
        let a: u128 = assets.0;
        if a == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(mul_div_floor(a.into(), new_total_supply.into(), new_total_assets.into()).into())
    }

    /// Converts an amount of shares to underlying assets, flooring the result.
    /// Uses virtual offsets and fee-aware totals (pre-accrual simulation).
    pub fn convert_to_assets(&self, shares: U128) -> U128 {
        let s: u128 = shares.0;
        if s == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(mul_div_floor(s.into(), new_total_assets.into(), new_total_supply.into()).into())
    }

    /// Preview the number of shares minted for a deposit of `assets` (floored).
    /// Simulates fee accrual first (minting fee shares), then applies virtual offsets for conversion.
    pub fn preview_deposit(&self, assets: U128) -> U128 {
        self.convert_to_shares(assets)
    }

    /// Preview the amount of assets required to mint `shares` (ceiled).
    /// Simulates fee accrual first (minting fee shares), then applies virtual offsets for conversion.
    pub fn preview_mint(&self, shares: U128) -> U128 {
        let s = shares.0;
        if s == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(mul_div_ceil(s.into(), new_total_assets.into(), new_total_supply.into()).into())
    }

    /// Preview the number of shares required to withdraw `assets` (ceiled).
    /// Applies virtual offsets and fee-aware totals (pre-accrual simulation).
    pub fn preview_withdraw(&self, assets: U128) -> U128 {
        let a = assets.0;
        if a == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(mul_div_ceil(a.into(), new_total_supply.into(), new_total_assets.into()).into())
    }

    /// Preview the amount of assets received by redeeming `shares` (floored).
    /// Returns 0 if total supply is zero.
    pub fn preview_redeem(&self, shares: U128) -> U128 {
        self.convert_to_assets(shares)
    }

    pub fn get_withdrawing_op_id(&self) -> Option<U64> {
        match &self.op_state {
            OpState::Withdrawing(WithdrawingState { op_id, .. }) => Some((*op_id).into()),
            _ => None,
        }
    }

    /// Returns `true` if any market execution lock is currently held.
    ///
    /// This is a coarse signal that a withdrawal or allocator-only
    /// rebalance is in-flight against at least one market.
    pub fn has_pending_market_withdrawal(&self) -> bool {
        self.market_execution_lock
            .inner()
            .active_len(TimestampNs(env::block_timestamp()))
            > 0
    }

    pub fn get_current_withdraw_request_id(&self) -> Option<U64> {
        match &self.op_state {
            OpState::Withdrawing(_) | OpState::Payout(_) => {
                Some(self.withdraw_queue.next_withdraw_to_execute.into())
            }
            _ => None,
        }
    }

    pub fn queue_tail(&self) -> u64 {
        self.withdraw_queue.next_pending_withdrawal_id
    }

    pub fn peek_next_pending_withdrawal_id(&self) -> Option<u64> {
        if let Some((id, _)) = self.withdraw_queue.head() {
            Event::WithdrawQueueStatus {
                status: QueueStatus::NextFound,
                id: Some(id.into()),
            }
            .emit();
            Some(id)
        } else {
            Event::WithdrawQueueStatus {
                status: QueueStatus::Empty,
                id: None,
            }
            .emit();
            None
        }
    }

    fn resolve_account(&self, address: &Address) -> AccountId {
        self.address_book
            .get(address)
            .cloned()
            .unwrap_or_else(|| panic_with_message(ERR_MISSING_WITHDRAWAL_QUEUE_ADDRESS))
    }

    pub(crate) fn set_op_state(&mut self, state: OpState) {
        self.op_state = state;
        self.rebuild_live_address_book();
    }

    fn apply_kernel_op_state(&mut self, state: &OpState) {
        self.set_op_state(state.clone());
    }

    pub fn build_real_assets_report(&self) -> RealAssetsReport {
        let per_market = self
            .markets
            .iter()
            .map(|(id, rec)| (*id, U128(rec.principal)))
            .collect();
        RealAssetsReport {
            total_assets: self.get_total_assets(),
            per_market,
            refreshed_at: env::block_timestamp().into(),
        }
    }

    pub fn get_market_id_of_account(&self, market: AccountId) -> Option<MarketId> {
        self.market_id_of(&market)
    }

    pub fn get_market_account_by_id(&self, market_id: U64) -> Option<AccountId> {
        u32::try_from(market_id.0)
            .ok()
            .map(MarketId::from)
            .and_then(|market_id| self.market_account_by_id(market_id).cloned())
    }

    pub fn list_markets_with_ids(&self) -> Vec<(U64, AccountId)> {
        self.markets
            .iter()
            .map(|(id, rec)| (U64::from(u64::from(u32::from(*id))), rec.account.clone()))
            .collect()
    }
}

/* ----- Private Helpers ----- */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleCoverage {
    pub remaining_unmet: u128,
    pub collected_from_idle: u128,
}

#[derive(Clone)]
struct SupplyQueueMarketInfo {
    cap_room: u128,
    cap_group_id: Option<CapGroupId>,
}

impl Contract {
    fn remember_account_mapping(&mut self, address: Address, account: AccountId) {
        self.address_book.entry(address).or_insert(account);
    }

    fn rebuild_live_address_book(&mut self) {
        let existing = std::mem::take(&mut self.address_book);
        let mut live = BTreeMap::new();

        let mut retain = |address: Address| {
            let account = existing
                .get(&address)
                .cloned()
                .unwrap_or_else(|| panic_with_message(ERR_MISSING_WITHDRAWAL_QUEUE_ADDRESS));
            live.entry(address).or_insert(account);
        };

        for pending in self.withdraw_queue.pending_withdrawals().values() {
            retain(pending.owner);
            retain(pending.receiver);
        }

        match &self.op_state {
            OpState::Withdrawing(state) => {
                retain(state.owner);
                retain(state.receiver);
            }
            OpState::Payout(state) => {
                retain(state.owner);
                retain(state.receiver);
            }
            _ => {}
        }

        self.address_book = live;
    }

    fn default_cap_group_record() -> CapGroupRecord {
        CapGroupRecord {
            cap: templar_curator_primitives::CapGroup::builder()
                .absolute_cap(0)
                .relative_cap(Wad::one())
                .build(),
            principal: 0,
        }
    }

    fn principal_of(&self, market_id: MarketId) -> u128 {
        self.market_record_by_id(market_id)
            .map_or(0, |r| r.principal)
    }

    fn market_cap_group_id(&self, market_id: MarketId) -> Option<CapGroupId> {
        self.market_record_by_id(market_id)
            .and_then(|r| r.cfg.cap_group_id.clone())
    }

    fn total_assets_for_caps(&self) -> u128 {
        // For relative caps we want “true” AUM. During allocation we temporarily
        // decrement `idle_balance` to reserve funds, so include `Allocating.remaining`.
        let mut total = self.get_total_assets().0;
        if let OpState::Allocating(s) = &self.op_state {
            total = total.saturating_add(s.remaining);
        }
        total
    }

    fn supply_queue_market_infos(&self) -> Vec<SupplyQueueMarketInfo> {
        self.supply_queue
            .iter()
            .filter_map(|market_id| {
                let rec = self.market_record_by_id(*market_id)?;
                Some(SupplyQueueMarketInfo {
                    cap_room: rec.cfg.cap.0.saturating_sub(rec.principal),
                    cap_group_id: rec.cfg.cap_group_id.clone(),
                })
            })
            .collect()
    }

    fn cap_group_room_remaining_at(&self, cap_group: &CapGroupId, total_assets: u128) -> u128 {
        let Some(rec) = self.cap_groups.get(cap_group) else {
            return 0;
        };

        rec.available_capacity(total_assets)
    }

    fn cap_group_room_remaining(&self, cap_group: &CapGroupId) -> u128 {
        self.cap_group_room_remaining_at(cap_group, self.total_assets_for_caps())
    }

    fn max_single_market_room_in_group(
        &self,
        cap_group: &CapGroupId,
        cap_room: u128,
        base_total_assets: u128,
    ) -> u128 {
        let mut low = 0u128;
        let mut high = cap_room;

        while low < high {
            let diff = high.saturating_sub(low);
            let mid = low.saturating_add(diff.saturating_add(1) / 2);

            let total_assets = base_total_assets.saturating_add(mid);
            let group_room = self.cap_group_room_remaining_at(cap_group, total_assets);
            let room = cap_room.min(group_room);

            if mid <= room {
                low = mid;
            } else {
                high = mid.saturating_sub(1);
            }
        }

        low
    }

    fn max_allocatable_room_at_precomputed(
        &self,
        total_assets: u128,
        markets: &[SupplyQueueMarketInfo],
    ) -> u128 {
        let mut total = 0u128;
        let mut group_remaining: BTreeMap<CapGroupId, u128> = BTreeMap::new();

        for market in markets {
            let market_room = market.cap_room;
            if market_room == 0 {
                continue;
            }

            if let Some(group_id) = market.cap_group_id.as_ref() {
                let entry = group_remaining
                    .entry(group_id.clone())
                    .or_insert_with(|| self.cap_group_room_remaining_at(group_id, total_assets));
                if *entry == 0 {
                    continue;
                }
                let room = market_room.min(*entry);
                total = total.saturating_add(room);
                *entry = entry.saturating_sub(room);
            } else {
                total = total.saturating_add(market_room);
            }
        }

        total
    }

    fn relative_cap_rounding_slack(&self) -> u128 {
        let mut groups = HashSet::<CapGroupId>::new();

        for market in self.supply_queue.iter() {
            let Some(group_id) = self.market_cap_group_id(*market) else {
                continue;
            };
            let Some(rec) = self.cap_groups.get(&group_id) else {
                continue;
            };

            if templar_curator_primitives::cap_group_record_absolute_cap(rec) == 0 {
                continue;
            }

            if templar_curator_primitives::cap_group_record_relative_cap(rec) < Wad::one() {
                groups.insert(group_id);
            }
        }

        u128::from(groups.len() as u64)
    }

    fn room_of(&self, market_id: MarketId) -> u128 {
        let Some(rec) = self.market_record_by_id(market_id) else {
            return 0;
        };

        let market_room = rec.cfg.cap.0.saturating_sub(rec.principal);
        if market_room == 0 {
            return 0;
        }

        if let Some(cap_group) = rec.cfg.cap_group_id.as_ref() {
            return market_room.min(self.cap_group_room_remaining(cap_group));
        }

        market_room
    }

    pub(crate) fn update_cap_group_principal(
        &mut self,
        cap_group: &CapGroupId,
        old: u128,
        new: u128,
    ) {
        if old == new {
            return;
        }
        let entry = self
            .cap_groups
            .entry(cap_group.clone())
            .or_insert_with(Self::default_cap_group_record);
        if new >= old {
            entry.principal = entry.principal.saturating_add(new - old);
        } else {
            entry.principal = entry.principal.saturating_sub(old - new);
        }
        Event::CapGroupPrincipalUpdated {
            cap_group: cap_group.clone(),
            principal: U128(entry.principal),
        }
        .emit();
    }

    pub(crate) fn set_market_principal(&mut self, market_id: MarketId, new_principal: u128) {
        let Some(rec) = self.market_record_by_id_mut(market_id) else {
            return;
        };

        let old = rec.principal;
        if old == new_principal {
            return;
        }

        rec.principal = new_principal;
        if let Some(cap_group) = rec.cfg.cap_group_id.clone() {
            self.update_cap_group_principal(&cap_group, old, new_principal);
        }
    }

    fn market_id_of(&self, market: &AccountId) -> Option<MarketId> {
        self.market_ids.get(market).copied()
    }

    fn market_id_of_or_panic(&self, market: &AccountId) -> MarketId {
        self.market_id_of(market)
            .unwrap_or_else(|| panic_with_message(&format!("Unknown market: {market}")))
    }

    fn market_account_by_id(&self, market_id: MarketId) -> Option<&AccountId> {
        self.markets.get(&market_id).map(|rec| &rec.account)
    }

    fn market_account_by_id_or_panic(&self, market_id: MarketId) -> &AccountId {
        self.market_account_by_id(market_id)
            .unwrap_or_else(|| panic_with_message(&format!("Unknown market: {market_id}")))
    }

    fn market_record_by_id(&self, market_id: MarketId) -> Option<&MarketRecord> {
        self.markets.get(&market_id)
    }

    fn market_record_by_id_or_panic(&self, market_id: MarketId) -> &MarketRecord {
        self.market_record_by_id(market_id)
            .unwrap_or_else(|| panic_with_message(&format!("Unknown market: {market_id}")))
    }

    fn market_record_by_id_mut(&mut self, market_id: MarketId) -> Option<&mut MarketRecord> {
        self.markets.get_mut(&market_id)
    }

    fn market_record_by_id_mut_or_panic(&mut self, market_id: MarketId) -> &mut MarketRecord {
        self.market_record_by_id_mut(market_id)
            .unwrap_or_else(|| panic_with_message(&format!("Unknown market: {market_id}")))
    }

    fn insert_market_record(&mut self, market_id: MarketId, record: MarketRecord) {
        self.market_ids.insert(record.account.clone(), market_id);
        self.markets.insert(market_id, record);
    }

    fn allocate_market_id(&mut self) -> MarketId {
        let id = MarketId::from(self.next_market_id);
        self.next_market_id = self
            .next_market_id
            .checked_add(1)
            .unwrap_or_else(|| panic_with_message("market id overflow"));
        id
    }

    #[cfg(test)]
    pub(crate) fn insert_market_for_tests(
        &mut self,
        market: AccountId,
        cfg: MarketConfiguration,
        principal: u128,
    ) -> MarketId {
        let id = self.allocate_market_id();
        let record = MarketRecord::with_parts(market.clone(), cfg, principal);
        self.insert_market_record(id, record);
        id
    }

    #[cfg(test)]
    pub(crate) fn insert_pending_withdrawal_for_tests(
        &mut self,
        id: u64,
        entry: PendingWithdrawal,
    ) {
        let owner_addr = account_id_to_address(&entry.owner);
        self.remember_account_mapping(owner_addr, entry.owner.clone());
        let receiver_addr = account_id_to_address(&entry.receiver);
        self.remember_account_mapping(receiver_addr, entry.receiver.clone());

        let mut pending = self.withdraw_queue.pending_withdrawals().clone();
        pending.insert(
            id,
            templar_vault_kernel::PendingWithdrawal::new(
                owner_addr,
                receiver_addr,
                entry.escrow_shares,
                entry.expected_assets,
                TimestampNs(entry.requested_at),
            ),
        );

        let next_pending_withdrawal_id = self
            .withdraw_queue
            .next_pending_withdrawal_id
            .max(id.saturating_add(1));
        let next_withdraw_to_execute = pending
            .keys()
            .next()
            .copied()
            .unwrap_or(next_pending_withdrawal_id);

        self.withdraw_queue = templar_vault_kernel::WithdrawQueue::with_state(
            pending.iter().map(|(id, w)| (*id, w.clone())),
            next_withdraw_to_execute,
            next_pending_withdrawal_id,
        );
        self.rebuild_live_address_book();
    }

    #[cfg(test)]
    pub(crate) fn pending_withdrawals_len(&self) -> usize {
        self.withdraw_queue.pending_withdrawals().len()
    }

    /// Computes fee-aware effective totals for conversions, mimicking `MetaMorpho`:
    /// - Include fee shares that would be minted if fees accrued now.
    /// - Apply virtual offsets: +`virtual_shares` to supply and +`virtual_assets` to assets.
    fn effective_totals_fee_aware(&self) -> (u128, u128) {
        let cur_total_assets = self.get_total_assets().0;
        let now = env::block_timestamp();
        let ts = self.total_supply();
        let anchor = &self.fee_anchor;

        let fee_total_assets = self.total_assets_for_fee_accrual(cur_total_assets, anchor, now);

        let mgmt_shares = self.compute_management_fee_shares(
            fee_total_assets,
            cur_total_assets,
            ts,
            anchor.timestamp_ns.0,
            now,
        );
        let ts_after_mgmt = Number::from(ts).saturating_add(mgmt_shares);

        let profit = fee_total_assets.saturating_sub(anchor.total_assets.0);
        let fee_assets = self.fees.performance.fee.apply_floored(profit.into());
        let performance_shares =
            compute_fee_shares_from_assets(fee_assets, cur_total_assets.into(), ts_after_mgmt);

        let new_total_supply = ts_after_mgmt
            .saturating_add(performance_shares)
            .saturating_add(self.virtual_shares.into());
        let new_total_assets =
            Number::from(cur_total_assets).saturating_add(self.virtual_assets.into());

        (new_total_supply.into(), new_total_assets.into())
    }

    pub fn compute_effective_totals(
        cur_assets: Number,
        last_total_assets: Number,
        performance_fee: Wad,
        total_supply: Number,
        virtual_shares: Number,
        virtual_assets: Number,
    ) -> (Number, Number) {
        let fee_shares =
            compute_fee_shares(cur_assets, last_total_assets, performance_fee, total_supply);
        // Bump by fake virtual assets to bypass inflation attacks
        let new_total_supply = total_supply
            .saturating_add(fee_shares)
            .saturating_add(virtual_shares);
        let new_total_assets = cur_assets.saturating_add(virtual_assets);
        (new_total_supply, new_total_assets)
    }

    fn total_assets_for_fee_accrual(
        &self,
        cur_total_assets: u128,
        anchor: &FeeAccrualAnchor,
        now: u64,
    ) -> u128 {
        let Some(max_rate) = self.fees.max_total_assets_growth_rate else {
            return cur_total_assets;
        };

        let anchor_assets = anchor.total_assets.0;

        // Only clamp *positive* growth; otherwise (loss/no-change), leave as-is.
        // Also ignore the limiter if the anchor is zero (avoid freezing at 0),
        // or if time goes backwards.
        if cur_total_assets <= anchor_assets || anchor_assets == 0 || now < anchor.timestamp_ns.0 {
            return cur_total_assets;
        }

        let elapsed_ns = now - anchor.timestamp_ns.0;
        if elapsed_ns == 0 {
            return anchor_assets;
        }

        // Cap growth with an annualized max rate.
        let annual_max_increase = max_rate.apply_floored(anchor_assets.into());
        let max_increase = mul_div_floor(
            annual_max_increase,
            Number::from(u128::from(elapsed_ns)),
            Number::from(u128::from(YEAR_NS)),
        )
        .as_u128_saturating();

        let max_total_assets = anchor_assets.saturating_add(max_increase);
        cur_total_assets.min(max_total_assets)
    }

    fn compute_management_fee_shares(
        &self,
        fee_assets_base: u128,
        cur_total_assets: u128,
        total_supply: u128,
        last_timestamp_ns: u64,
        now: u64,
    ) -> Number {
        if self.fees.management.fee.is_zero() || total_supply == 0 || now <= last_timestamp_ns {
            return Number::zero();
        }
        let elapsed_ns = now - last_timestamp_ns;
        let annual_fee_assets = self
            .fees
            .management
            .fee
            .apply_floored(fee_assets_base.into());
        let fee_assets = mul_div_floor(
            annual_fee_assets,
            Number::from(u128::from(elapsed_ns)),
            Number::from(u128::from(YEAR_NS)),
        );
        compute_fee_shares_from_assets(fee_assets, cur_total_assets.into(), total_supply.into())
    }

    pub fn clamp_allocation_total(&self, requested: Option<u128>) -> u128 {
        let requested = requested.unwrap_or(self.idle_balance);
        let max_room = self.get_max_deposit().0;
        requested.min(self.idle_balance).min(max_room)
    }

    pub fn internal_accrue_fee(&mut self) {
        let now = env::block_timestamp();
        let cur_total_assets = self.get_total_assets().0;
        let mut total_supply = self.total_supply();
        let anchor = self.fee_anchor.clone();

        // Cap the effective total_assets used for fee accrual to mitigate
        // donation-style AUM spikes within a short time window.
        let fee_total_assets = self.total_assets_for_fee_accrual(cur_total_assets, &anchor, now);

        let mgmt_shares = self.compute_management_fee_shares(
            fee_total_assets,
            cur_total_assets,
            total_supply,
            anchor.timestamp_ns.into(),
            now,
        );
        if mgmt_shares > Number::zero() {
            let minted: u128 = mgmt_shares.into();
            let recipient = self.fees.management.recipient.clone();
            let minted_res = self
                .mint(&Nep141Mint::new(minted, &recipient))
                .inspect_err(|e| {
                    Event::ManagementFeeMintFailed {
                        error: e.to_string(),
                    }
                    .emit();
                });
            if minted_res.is_ok() {
                Event::ManagementFeeAccrued {
                    recipient,
                    shares: U128(minted),
                }
                .emit();
            }
            total_supply = self.total_supply();
        }

        let profit = fee_total_assets.saturating_sub(anchor.total_assets.into());
        let fee_assets = self.fees.performance.fee.apply_floored(profit.into());
        let performance_shares = compute_fee_shares_from_assets(
            fee_assets,
            cur_total_assets.into(),
            total_supply.into(),
        );

        if performance_shares > Number::zero() {
            let minted: u128 = performance_shares.into();
            let recipient = self.fees.performance.recipient.clone();
            let minted_res = self
                .mint(&Nep141Mint::new(minted, &recipient))
                .inspect_err(|e| {
                    Event::PerformanceFeeMintFailed {
                        error: e.to_string(),
                    }
                    .emit();
                });
            if minted_res.is_ok() {
                Event::PerformanceFeeAccrued {
                    recipient,
                    shares: U128(minted),
                }
                .emit();
            }
        }

        if now > anchor.timestamp_ns.0 {
            self.apply_kernel_refresh_fees(now, cur_total_assets);
        } else {
            self.fee_anchor.total_assets = cur_total_assets.into();
        }
    }

    fn apply_kernel_refresh_fees(&mut self, now: u64, cur_total_assets: u128) {
        let kernel_state = self.kernel_state_mirror();
        let kernel_config = self.kernel_config_mirror();
        let kernel_restrictions = self.kernel_restrictions_mirror();
        let self_address = account_id_to_address(&env::current_account_id());

        let result = apply_action(
            kernel_state,
            &kernel_config,
            kernel_restrictions.as_ref(),
            &self_address,
            KernelAction::RefreshFees {
                now_ns: TimestampNs(now),
            },
        )
        .unwrap_or_else(|_| panic_with_message("Kernel refresh fees failed"));

        // Anchor updates to the *actual* AUM snapshot, so the max-rate limiter
        // only affects what can be charged as fees for the elapsed interval.
        self.fee_anchor = FeeAccrualAnchor {
            total_assets: cur_total_assets.into(),
            timestamp_ns: u64::from(result.state.fee_anchor.timestamp_ns).into(),
        };
    }

    fn apply_kernel_pause(&self, paused: bool) {
        let kernel_state = self.kernel_state_mirror();
        let kernel_config = self.kernel_config_mirror();
        let kernel_restrictions = self.kernel_restrictions_mirror();
        let self_address = account_id_to_address(&env::current_account_id());

        let _ = apply_action(
            kernel_state,
            &kernel_config,
            kernel_restrictions.as_ref(),
            &self_address,
            KernelAction::Pause { paused },
        )
        .unwrap_or_else(|_| panic_with_message("Kernel pause failed"));
    }

    /* ----- Internal: op orchestration ----- */
    fn ensure_idle(&self) {
        // Invariant: Only one op in flight; ensure_idle() guards all mutating ops.
        if !matches!(self.op_state, OpState::Idle) {
            templar_common::panic_with_message(
                "Invariant: Only one op in flight; current op_state != Idle",
            );
        }
    }

    fn start_allocation(&mut self, amount: u128, plan: AllocationPlan) -> PromiseOrValue<()> {
        if amount == 0 {
            return PromiseOrValue::Value(());
        }

        self.ensure_idle();

        let op_id = self.next_op_id;
        self.next_op_id = self.next_op_id.saturating_add(1);

        let kernel_plan = plan
            .iter()
            .map(|(market, amount)| AllocationPlanEntry::new(market.into_target_id(), *amount))
            .collect();
        let kernel_state = self.kernel_state_mirror();
        let kernel_config = self.kernel_config_mirror();
        let kernel_restrictions = self.kernel_restrictions_mirror();
        let self_addr = account_id_to_address(&env::current_account_id());

        // Kernel handles idle_assets validation and decrement in BeginAllocating.
        let result = apply_action(
            kernel_state,
            &kernel_config,
            kernel_restrictions.as_ref(),
            &self_addr,
            KernelAction::BeginAllocating {
                op_id,
                plan: kernel_plan,
                now_ns: TimestampNs(env::block_timestamp()),
            },
        )
        .unwrap_or_else(|_| panic_with_message("Kernel begin allocation failed"));

        self.idle_balance = result.state.idle_assets;
        self.apply_kernel_op_state(&result.state.op_state);
        self.next_op_id = result.state.next_op_id;

        Event::AllocationStarted {
            op_id: op_id.into(),
            remaining: U128(amount),
        }
        .emit();

        self.step_allocation()
    }

    /// build a supply `transfer_call` and chain `after_supply_1_check`
    fn supply_and_then(
        &self,
        market_id: MarketId,
        amount: u128,
        op_id: u64,
        index: u32,
        remaining_before: u128,
    ) -> Promise {
        self::require_at_least(
            SUPPLY_AFTER_TRANSFER_CHECK_GAS.saturating_add(GAS_FOR_FT_TRANSFER_CALL),
        );

        let market = self.market_account_by_id_or_panic(market_id);

        self.underlying_asset
            .transfer_call(
                market,
                U128(amount).into(),
                Some(
                    #[allow(clippy::expect_used, reason = "Infallible")]
                    serde_json::to_string(&templar_common::market::DepositMsg::Supply)
                        .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()))
                        .as_str(),
                ),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(SUPPLY_AFTER_TRANSFER_CHECK_GAS)
                    .supply_01_handle_transfer(
                        market_id,
                        op_id,
                        index,
                        U128(amount),
                        U128(remaining_before),
                    ),
            )
    }

    fn step_allocation(&mut self) -> PromiseOrValue<()> {
        let (op_id, index, remaining, plan) = match &self.op_state {
            OpState::Allocating(AllocatingState {
                op_id,
                index,
                remaining,
                plan,
            }) => (*op_id, *index, *remaining, plan.clone()),
            _ => return self.stop_and_exit(Some(&Error::NotAllocating)),
        };

        if remaining == 0 {
            return self.stop_and_exit::<Error>(None);
        }

        let idx = index as usize;
        if let Some(step) = plan.get(idx) {
            let market_id = MarketId::from(step.target_id);

            let room = self.room_of(market_id);
            let to_supply = room.min(step.amount);

            Event::AllocationStepPlan {
                op_id: op_id.into(),
                index,
                market: market_id,
                target: U128(step.amount),
                room: U128(room),
                to_supply: U128(to_supply),
                remaining_before: U128(remaining),
                planned: true,
                reason: None,
            }
            .emit();

            if to_supply == 0 {
                Event::AllocationStepPlan {
                    op_id: op_id.into(),
                    index,
                    market: market_id,
                    target: U128(step.amount),
                    room: U128(room),
                    to_supply: U128(0),
                    remaining_before: U128(remaining),
                    planned: false,
                    reason: Some(if room == 0 {
                        Reason::NoRoom
                    } else {
                        Reason::ZeroTarget
                    }),
                }
                .emit();

                let kernel_state = self.op_state.clone();
                let result = templar_vault_kernel::transitions::allocation_step_callback(
                    kernel_state,
                    true,
                    0,
                    op_id,
                )
                .unwrap_or_else(|_| panic_with_message("Kernel allocation step failed"));
                self.apply_kernel_op_state(&result.new_state);
                return self.step_allocation();
            }

            PromiseOrValue::Promise(
                self.supply_and_then(market_id, to_supply, op_id, index, remaining),
            )
        } else {
            // Plan exhausted; stop and reconcile remaining in stop_and_exit
            self.stop_and_exit::<Error>(None)
        }
    }

    fn start_withdraw(
        &mut self,
        amount: u128,
        receiver: &AccountId,
        owner: &AccountId,
        escrow_shares: u128,
        route: Vec<MarketId>,
    ) -> PromiseOrValue<()> {
        if amount == 0 {
            return self.stop_and_exit(Some(&Error::ZeroAmount));
        }

        {
            use templar_curator_primitives::find_first_duplicate;

            let ids: Vec<u32> = route.iter().map(IntoTargetId::into_target_id).collect();
            if let Some(dup) = find_first_duplicate(&ids) {
                use crate::convert::IntoMarketId;
                panic_with_message(&format!(
                    "Duplicate market in withdraw route: {}",
                    dup.into_market_id()
                ));
            }
        }

        self.ensure_idle();

        let op_id = self.next_op_id;
        self.next_op_id = self.next_op_id.saturating_add(1);

        // Policy: Idle-first reservation does not mutate idle_balance until payout succeeds.
        let cov = self.compute_idle_coverage(amount);

        self.withdraw_route = route.into();

        let request = templar_vault_kernel::transitions::WithdrawalRequest {
            op_id,
            request_id: self.withdraw_queue.next_withdraw_to_execute,
            amount,
            receiver: account_id_to_address(receiver),
            owner: account_id_to_address(owner),
            escrow_shares,
        };
        let kernel_state = self.op_state.clone();
        let mut result = templar_vault_kernel::transitions::start_withdrawal(kernel_state, request)
            .unwrap_or_else(|_| panic_with_message("Kernel start withdrawal failed"));

        if cov.collected_from_idle > 0 {
            result = templar_vault_kernel::transitions::withdrawal_step_callback(
                result.new_state,
                op_id,
                cov.collected_from_idle,
            )
            .unwrap_or_else(|_| panic_with_message("Kernel idle withdraw step failed"));
        }

        self.apply_kernel_op_state(&result.new_state);
        self.pay_or_signal_next_withdraw()
    }

    fn pay_or_signal_next_withdraw(&mut self) -> PromiseOrValue<()> {
        let OpState::Withdrawing(WithdrawingState {
            op_id,
            request_id: _,
            index,
            remaining,
            receiver,
            collected,
            owner,
            escrow_shares,
        }) = self.op_state.clone()
        else {
            return self.stop_and_exit(Some(&Error::NotWithdrawing));
        };

        if remaining == 0 {
            Event::WithdrawProgress {
                phase: WithdrawProgressPhase::CoveredByIdle,
                op_id: Some(op_id.into()),
                id: Some(self.withdraw_queue.next_withdraw_to_execute.into()),
                market: None,
                owner: None,
                receiver: None,
                escrow_shares: None,
                expected_assets: None,
                requested_at: None,
            }
            .emit();
            return self.pay(
                op_id,
                &receiver,
                collected,
                &owner,
                escrow_shares,
                escrow_shares,
            );
        }
        if let Some(market) = self.withdraw_route.get(index as usize).copied() {
            Event::WithdrawProgress {
                phase: WithdrawProgressPhase::ExecutionRequired,
                op_id: Some(op_id.into()),
                id: Some(self.withdraw_queue.next_withdraw_to_execute.into()),
                market: Some(market),
                owner: None,
                receiver: None,
                escrow_shares: None,
                expected_assets: None,
                requested_at: None,
            }
            .emit();
            PromiseOrValue::Value(())
        } else {
            let requested = collected.saturating_add(remaining);
            let burn_shares = compute_idle_settlement(escrow_shares, requested, collected)
                .map_or(0, |result| result.settlement.to_burn);

            self.pay_or_else(
                op_id,
                &receiver,
                collected,
                &owner,
                escrow_shares,
                burn_shares,
                |self_| {
                    let failed_route = std::mem::take(&mut self_.withdraw_route);
                    self_.op_state = OpState::Idle;
                    self_.park_head_for_retry(failed_route);
                    PromiseOrValue::Value(())
                },
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// If we collected something, pay it out now and burn proportional shares or do something else
    fn pay_or_else(
        &mut self,
        op_id: u64,
        receiver: &Address,
        amount: u128,
        owner: &Address,
        escrow_shares: u128,
        burn_shares: u128,
        or_else: impl FnOnce(&mut Self) -> PromiseOrValue<()>,
    ) -> PromiseOrValue<()> {
        if amount > 0 {
            self.pay(op_id, receiver, amount, owner, escrow_shares, burn_shares)
        } else {
            or_else(self)
        }
    }

    fn pay(
        &mut self,
        op_id: u64,
        receiver: &Address,
        amount: u128,
        owner: &Address,
        escrow_shares: u128,
        burn_shares: u128,
    ) -> PromiseOrValue<()> {
        let receiver_account = self.resolve_account(receiver);
        let withdrawing = unwrap_or_return!(crate::impl_callbacks::or_stop(self, op_id));

        let mut payout = withdrawing.into_payout(PayoutState {
            op_id,
            request_id: withdrawing.request_id,
            receiver: *receiver,
            amount,
            owner: *owner,
            escrow_shares,
            burn_shares,
        });

        require!(payout.idle_balance >= amount, "idle underflow in payout");
        payout.update_idle_balance(IdleBalanceDelta::Decrease(amount.into()));

        PromiseOrValue::Promise(
            payout
                .underlying_asset
                .transfer(receiver_account.clone(), U128(amount).into())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(AFTER_SEND_TO_USER_GAS)
                        .payment_01_reconcile_idle_or_refund(op_id, receiver_account, U128(amount)),
                ),
        )
    }

    /// Computes how much of `amount` can be covered by idle balance without mutating state.
    /// Returns `IdleCoverage`.
    fn compute_idle_coverage(&self, amount: u128) -> IdleCoverage {
        let used_idle = self.idle_balance.min(amount);
        IdleCoverage {
            remaining_unmet: amount.saturating_sub(used_idle),
            collected_from_idle: used_idle,
        }
    }

    fn pop_head(&mut self) {
        let removed = self.withdraw_queue.dequeue();
        let Some((id, _)) = removed else {
            panic_with_message("queue corrupt: head missing");
        };
        Event::WithdrawQueueUpdate {
            action: QueueAction::Dequeued,
            id: id.into(),
        }
        .emit();
        self.rebuild_live_address_book();
    }

    fn park_head_for_retry(&mut self, failed_route: WithdrawRoute) {
        let failed_route: Vec<MarketId> = failed_route.into();
        Event::WithdrawQueueUpdate {
            action: QueueAction::Parked,
            id: self.withdraw_queue.next_withdraw_to_execute.into(),
        }
        .emit();

        Event::WithdrawParkedDetail {
            id: self.withdraw_queue.next_withdraw_to_execute.into(),
            failed_route,
            reason: Reason::RouteExhaustedNoFunds,
        }
        .emit();
    }

    fn refresh_targets(&self, mut markets: Vec<MarketId>) -> Vec<MarketId> {
        if markets.is_empty() {
            return self.markets.keys().copied().collect();
        }
        markets.retain(|m| self.market_record_by_id(*m).is_some());
        markets
    }

    fn refresh_step(&mut self, op_id: u64) -> PromiseOrValue<RealAssetsReport> {
        let (index, plan) = match &self.op_state {
            OpState::Refreshing(RefreshingState {
                op_id: cur,
                index,
                plan,
            }) if *cur == op_id => (*index, plan.clone()),
            _ => return PromiseOrValue::Value(self.build_real_assets_report()),
        };

        if index as usize >= plan.len() {
            let report = self.build_real_assets_report();
            self.last_refresh_ns = u64::from(report.refreshed_at);
            Event::RefreshCompleted {
                op_id: op_id.into(),
                markets: plan.into_iter().map(MarketId::from).collect(),
                total_assets: report.total_assets,
                refreshed_at: report.refreshed_at,
            }
            .emit();
            self.set_op_state(OpState::Idle);
            return PromiseOrValue::Value(report);
        }

        let market_id = MarketId::from(plan[index as usize]);
        let before = self.principal_of(market_id);

        let market_account = self.market_account_by_id_or_panic(market_id).clone();

        PromiseOrValue::Promise(
            ext_market::ext(market_account)
                .with_static_gas(GET_SUPPLY_POSITION_GAS)
                .with_unused_gas_weight(0)
                .get_supply_position(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(SUPPLY_POSITION_READ_CALLBACK_GAS)
                        .refresh_01_settle(market_id, op_id, index, U128(before)),
                ),
        )
    }
}

impl OldContract {
    fn into_current(self) -> Contract {
        let legacy_locks = {
            use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
            let mut bytes = Vec::new();
            self.market_execution_lock
                .serialize(&mut bytes)
                .unwrap_or_else(|_| panic_with_message("Failed to serialize legacy Locker"));
            Vec::<MarketId>::try_from_slice(&bytes)
                .unwrap_or_else(|_| panic_with_message("Failed to decode legacy Locker"))
        };

        let market_execution_lock =
            MarketExecutionLock::from_markets(legacy_locks, env::block_timestamp());

        let mut address_book = BTreeMap::new();
        let mut pending_withdrawals = BTreeMap::new();
        let mut max_id = 0u64;

        for (id, entry) in self.pending_withdrawals.iter() {
            let owner_addr = account_id_to_address(&entry.owner);
            address_book
                .entry(owner_addr)
                .or_insert_with(|| entry.owner.clone());
            let receiver_addr = account_id_to_address(&entry.receiver);
            address_book
                .entry(receiver_addr)
                .or_insert_with(|| entry.receiver.clone());

            pending_withdrawals.insert(
                *id,
                templar_vault_kernel::PendingWithdrawal::new(
                    owner_addr,
                    receiver_addr,
                    entry.escrow_shares,
                    entry.expected_assets,
                    TimestampNs(entry.requested_at),
                ),
            );

            max_id = max_id.max(*id);
        }

        let next_pending_withdrawal_id = if pending_withdrawals.is_empty() {
            self.next_withdraw_to_execute
        } else {
            max_id.saturating_add(1).max(self.next_withdraw_to_execute)
        };

        let withdraw_queue = templar_vault_kernel::WithdrawQueue::with_state(
            pending_withdrawals,
            self.next_withdraw_to_execute,
            next_pending_withdrawal_id,
        );

        if !withdraw_queue.pending_withdrawals().is_empty()
            && !withdraw_queue
                .pending_withdrawals()
                .contains_key(&withdraw_queue.next_withdraw_to_execute)
        {
            panic_with_message("withdraw queue head missing during migration");
        }

        Contract {
            underlying_asset: self.underlying_asset,
            aum: self.aum,
            fees: self.fees,
            skim_recipient: self.skim_recipient,
            fee_anchor: self.fee_anchor,
            idle_balance: self.idle_balance,
            op_state: self.op_state,
            next_op_id: self.next_op_id,
            last_refresh_ns: self.last_refresh_ns,
            refresh_cooldown_ns: self.refresh_cooldown_ns,
            withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
            idle_resync_last_ns: self.idle_resync_last_ns,
            idle_resync_cooldown_ns: self.idle_resync_cooldown_ns,
            idle_resync_inflight_op_id: self.idle_resync_inflight_op_id,
            virtual_shares: self.virtual_shares,
            virtual_assets: self.virtual_assets,
            markets: self.markets,
            market_ids: self.market_ids,
            cap_groups: self.cap_groups,
            next_market_id: self.next_market_id,
            governance_timelocks: self.governance_timelocks,
            supply_queue: self.supply_queue.into(),
            withdraw_queue,
            address_book,
            market_execution_lock,
            withdraw_route: self.withdraw_route.into(),
            abdicator: self.abdicator,
            gate: self.gate,
        }
    }
}

impl near_sdk_contract_tools::hook::Hook<Self, Nep145ForceUnregister<'_>> for Contract {
    fn hook<R>(_: &mut Self, _: &Nep145ForceUnregister, _: impl FnOnce(&mut Self) -> R) -> R {
        // Invariant: Force unregister must fail to preserve FT ledger integrity.
        templar_common::panic_with_message("force unregistration is not supported")
    }
}

#[cfg(test)]
mod tests;
