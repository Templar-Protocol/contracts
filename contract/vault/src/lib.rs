#![allow(clippy::needless_pass_by_value)]

use crate::{
    aum::AUM,
    governance::Abdicator,
    governance::Gate,
    governance::Timelocks,
    storage_management::{require_attached_at_least, require_attached_for_pending_withdrawal},
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, serde_json,
    store::IterableMap,
    AccountId, BorshStorageKey, Gas, IntoStorageKey, NearToken, PanicOnDefault, Promise,
    PromiseOrValue,
};
use near_sdk_contract_tools::{
    ft::{
        nep141::GAS_FOR_FT_TRANSFER_CALL, nep145::Nep145ForceUnregister, ContractMetadata,
        FungibleToken, Nep141Controller, Nep141Mint, Nep141Transfer, Nep145 as _, Nep145Controller,
        Nep148Controller, StorageBalanceBounds,
    },
    Owner, Rbac,
};
use near_sdk_contract_tools::{owner::Owner, rbac};
use near_sdk_contract_tools::{owner::OwnerExternal, rbac::Rbac};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    num::NonZeroU8,
};
use templar_common::{
    asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset},
    market::ext_market,
    vault::{
        require_at_least, AllocatingState, AllocationMode, AllocationPlan, AllocationWeights,
        Error, Event, IdleBalanceDelta, MarketConfiguration, OpState, PayoutState,
        PendingWithdrawal, TimestampNs, VaultConfiguration, WithdrawingState,
        AFTER_CREATE_WITHDRAW_REQ_GAS, AFTER_SEND_TO_USER_GAS, AFTER_SUPPLY_1_CHECK_GAS,
        ALLOCATE_GAS, CREATE_WITHDRAW_REQ_GAS, EXECUTE_WITHDRAW_01_FETCH_POSITION_GAS,
        EXECUTE_WITHDRAW_GAS, MAX_QUEUE_LEN, MAX_TIMELOCK_NS, MIN_TIMELOCK_NS, WITHDRAW_GAS,
    },
};
pub use wad::*;

pub mod aum;
pub mod governance;
pub mod impl_callbacks;
pub mod impl_token_receiver;
pub mod storage_management;
pub mod wad;

#[cfg(test)]
mod test_utils;

#[derive(Debug, Clone)]
#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
/// Internal storage keys used by persistent collections.
pub enum StorageKey {
    PendingWithdrawals,
}

#[derive(BorshStorageKey)]
#[near]
/// Role-based access control roles for privileged actions.
pub enum Role {
    /// Primary operator for market configuration and policy.
    /// Can submit/accept cap changes and market removals, and is implicitly granted the Allocator role.
    Curator,
    /// Safety backstop that can revoke pending governance changes (e.g., timelock/guardian).
    /// Has no authority to change caps or the supply queue on its own.
    Guardian,
    /// Operational role for allocation and withdrawal execution.
    /// May set the supply_queue while the vault is Idle; cannot modify caps/timelocks/guardian.
    Allocator,
}

#[near(serializers = [borsh])]
#[derive(Debug, Clone, Default)]
pub struct MarketRecord {
    pub cfg: MarketConfiguration,
    pub principal: u128,
}

impl From<MarketConfiguration> for MarketRecord {
    fn from(cfg: MarketConfiguration) -> Self {
        Self { cfg, principal: 0 }
    }
}

#[derive(PanicOnDefault, FungibleToken, Owner, Rbac)]
#[fungible_token(force_unregister_hook = "Self")]
#[rbac(roles = "Role", crate = "crate")]
#[near(contract_state)]
/// Vault contract that issues shares over an underlying fungible asset and allocates liquidity
/// across configured markets. Implements 4626-like deposit/withdraw semantics.
///
/// What this contract does
/// - Issues a share token (NEP-141) that represents a vault over an underlying NEP-141 “BorrowAsset”.
/// - Allocates deposits across “markets” via a supply queue; withdrawals are keeper-routed via a queueless mechanism.
/// - Governance uses Owner + RBAC (Curator/Guardian/Allocator) with a timelock for certain changes.
/// - Withdraw flow escrows shares, builds market-side withdrawal requests, then pays out and burns proportional escrow.
/// - Performance fees accrue by minting fee shares based on increases in total assets.
/// Critical invariants
/// - Assets accounting is correct: total_assets = idle_balance + sum(all principals in markets).
/// - Only one op in flight (op_state); mutating ops require Idle.
/// - Governance changes obey timelocks; Guardian may revoke pending changes.
///
/// Note: RBAC storage is paid by the contract; callers are not charged deposits for RBAC changes.
pub struct Contract {
    /// The underlying asset that the vault manages
    underlying_asset: FungibleAsset<BorrowAsset>,

    /// The process in which the vault calculates its assets under management
    aum: AUM,

    /// Performance fee
    performance_fee: wad::Wad,
    fee_recipient: AccountId,
    skim_recipient: AccountId,
    /// Last recorded total assets (for fee accrual)
    last_total_assets: u128,

    // Virtual offsets used only in conversions/previews to harden edge cases
    virtual_shares: u128,
    virtual_assets: u128,

    // Merged market record: cfg + pending_cap + principal (single persisted map; no per-entry storage keys)
    markets: BTreeMap<AccountId, MarketRecord>,

    /// Per‑action governance timelock configuration.
    governance_timelocks: Timelocks,

    /// Ordered list of market IDs for deposit allocation
    supply_queue: BTreeSet<AccountId>,

    // id of the pending withdrawal being executed, if any
    current_withdraw_inflight: Option<u64>,

    /// underlying held by vault
    idle_balance: u128,
    op_state: OpState,
    next_op_id: u64,

    /// Pending withdrawals queue (vault-level, FIFO by id)
    pending_withdrawals: IterableMap<u64, PendingWithdrawal>,
    next_withdraw_id: u64,
    next_withdraw_to_execute: u64,

    // indices of markets with created requests (per withdrawing op)
    market_execution_lock: Locker,

    // Keeper-provided withdraw route for the current Withdrawing op
    withdraw_route: Vec<AccountId>,

    abdicator: Abdicator,
    gate: Gate,
}

#[near]
impl Contract {
    #[allow(clippy::unwrap_used, reason = "Infallible")]
    #[init]
    /// Initializes a new vault.
    /// - `owner_id`: account that controls Owner-only actions.
    /// - `curator_id`: manages markets and is also granted the Allocator role.
    /// - `guardian_id`: can revoke pending governance actions.
    /// - `underlying_token_id`: NEP-141 underlying asset managed by the vault.
    /// - `initial_timelock_sec`: governance timelock in seconds.
    /// - `fee_recipient`: account to receive performance fees.
    /// - `skim_recipient`: account to receive skimmed tokens.
    /// - `name`/`symbol`/`decimals`: metadata for the share token.
    #[must_use]
    pub fn new(configuration: VaultConfiguration) -> Self {
        let VaultConfiguration {
            owner,
            curator,
            guardian,
            underlying_token,
            initial_timelock_ns,
            fee_recipient,
            skim_recipient,
            name,
            symbol,
            decimals,
            restrictions,
        } = configuration;

        require!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&initial_timelock_ns.0),
            "timelock bounds"
        );

        let mut contract = Self {
            underlying_asset: underlying_token,
            aum: AUM::BalanceSheet,
            performance_fee: Wad::default(),
            fee_recipient,
            skim_recipient,
            markets: BTreeMap::new(),
            governance_timelocks: governance::Timelocks::new(
                initial_timelock_ns.0,
                initial_timelock_ns.0,
                initial_timelock_ns.0,
                initial_timelock_ns.0,
            ),
            supply_queue: BTreeSet::default(),
            last_total_assets: 0,
            virtual_shares: 1,
            virtual_assets: 1,
            idle_balance: 0,
            op_state: OpState::Idle,
            next_op_id: 1,
            current_withdraw_inflight: None,
            pending_withdrawals: IterableMap::new(
                [
                    b'v'.into_storage_key().as_slice(),
                    StorageKey::PendingWithdrawals.into_storage_key().as_slice(),
                ]
                .concat(),
            ),
            next_withdraw_id: 0,
            next_withdraw_to_execute: 0,
            market_execution_lock: Vec::new(),
            withdraw_route: Vec::new(),
            abdicator: Abdicator::new(),
            gate: Gate::new(restrictions),
        };

        contract.set_metadata(&ContractMetadata::new(name, symbol, decimals.into()));
        Owner::init(&mut contract, &owner);
        Rbac::add_role(&mut contract, &curator, &Role::Curator);
        Rbac::add_role(&mut contract, &curator, &Role::Allocator);
        Rbac::add_role(&mut contract, &guardian, &Role::Guardian);

        contract.set_storage_balance_bounds(&StorageBalanceBounds {
            min: NearToken::from_millinear(2),
            max: None,
        });
        contract
    }

    /// Burns the necessary shares to withdraw `amount` of underlying to `receiver`.
    /// Internally calls `redeem` after computing the share amount.
    #[payable]
    pub fn withdraw(&mut self, amount: U128, receiver: AccountId) -> PromiseOrValue<()> {
        require_at_least(WITHDRAW_GAS);
        let shares_needed = self.preview_withdraw(amount).0;
        self.redeem(U128(shares_needed), receiver)
    }

    /// Redeems `shares` for underlying assets sent to `receiver`.
    /// Shares are escrowed to the contract and only burned after successful payout.
    #[payable]
    pub fn redeem(&mut self, shares: U128, receiver: AccountId) -> PromiseOrValue<()> {
        let shares = shares.0;
        let assets = self.convert_to_assets(U128(shares)).0;
        let sender = env::predecessor_account_id();

        // Gate withdraw entrypoint: who is sending and who will receive assets.
        self.gate.enforce_policy(&sender);
        self.gate.enforce_policy(&receiver);

        require!(shares > 0, "Invalid shares");
        require!(assets > 0, "Dust redeem would yield 0 assets");

        let _ = require_attached_for_pending_withdrawal();

        Gate::bypass_transfer(
            self,
            &Nep141Transfer::new(shares, &sender, env::current_account_id()),
        );

        self.internal_accrue_fee();

        Event::RedeemRequested {
            shares: U128(shares),
            estimated_assets: U128(assets),
        }
        .emit();

        self.enqueue_pending_withdrawal(&sender, &receiver, shares, assets);
        PromiseOrValue::Value(())
    }

    /// Executes the next pending withdrawal request
    /// This defers creating market-side withdrawal requests until explicitly invoked.
    pub fn execute_next_withdrawal_request(&mut self, route: Vec<AccountId>) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        self.ensure_idle();
        Self::assert_allocator();

        if self.current_withdraw_inflight.is_some() {
            templar_common::panic_with_message("A pending withdrawal is already in-flight");
        }

        if let Some(id) = self.peek_next_pending_withdrawal_id() {
            let pending = self.pending_withdrawals.get(&id).unwrap_or_else(|| {
                templar_common::panic_with_message("pending vanished unexpectedly")
            });
            let owner = pending.owner.clone();
            let receiver = pending.receiver.clone();

            if pending.expected_assets == 0 {
                // Skip dust request to avoid wedging the queue
                self.current_withdraw_inflight = Some(id);
                self.remove_inflight_and_advance_head();
                return self.execute_next_withdrawal_request(route);
            }

            self.current_withdraw_inflight = Some(id);
            env::log_str(&format!("WithdrawalExecutionStarted id={id}"));
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

    /// Executes one created market withdrawal request in the current Withdrawing op.
    /// Allocator only.
    pub fn execute_next_market_withdrawal(
        &mut self,
        op_id: U64,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        Self::assert_allocator();

        let _ctx = match self.ctx_withdrawing(op_id.into()) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        let Some(market_index) = self.pending_market_exec.first().copied() else {
            templar_common::panic_with_message("No pending market withdrawal request to execute");
        };

        if let Err(e) = self.resolve_withdraw_market(market_index) {
            return self.stop_and_exit(Some(&e));
        };

        self.market_execution_lock.lock(market_index);
        PromiseOrValue::Promise(
            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                .with_static_gas(Gas::from_tgas(5))
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(EXECUTE_WITHDRAW_01_FETCH_POSITION_GAS)
                        .execute_withdraw_01_call_market_fetch_position(
                            op_id.into(),
                            market_index,
                            batch_limit,
                        ),
                ),
        )
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
    /// So in one allocation cycle, we should do at most ~12 market allocations.
    /// This is a conservative estimate, and may need to be tweaked.
    ///
    ///
    /// NOTE: When we rewrite this we should use a delta based approach
    pub fn reallocate(&mut self, delta: AllocationDelta) -> PromiseOrValue<()> {
        Self::assert_allocator();
        self.ensure_idle();
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
                        .cloned()
                        .map(|(market, amount)| (market, amount.into()))
                        .collect(),
                }
                .emit();

                self.start_allocation(total, plan)
            }
            AllocationDelta::Withdraw(delta) => {
                require_at_least(CREATE_WITHDRAW_REQ_GAS);

                let to_request = self.principal_of(&delta.market).min(delta.amount.0);
                require!(to_request > 0, "Insufficient principal");

                // TODO: proper event
                env::log_str(&format!(
                    "DeltaWithdrawRequestCreated market={} amount={}",
                    delta.market, to_request
                ));

                PromiseOrValue::Promise(
                    ext_market::ext(delta.market.clone())
                        .with_static_gas(CREATE_WITHDRAW_REQ_GAS)
                        .create_supply_withdrawal_request(BorrowAssetAmount::from(U128(
                            to_request,
                        ))),
                )
            }
            AllocationDelta::Harvest(delta) => todo!(),
        }
    }

    // Advance next_withdraw_to_execute to the next present id and return it, or None if none
    fn peek_next_pending_withdrawal_id(&mut self) -> Option<u64> {
        let mut id = self.next_withdraw_to_execute;
        while id < self.next_withdraw_id {
            if self.pending_withdrawals.get(&id).is_some() {
                self.next_withdraw_to_execute = id;
                return Some(id);
            }
            id = id.saturating_add(1);
        }
        self.next_withdraw_to_execute = id;
        None
    }

    // Remove the in-flight pending (success or explicit abort) and advance head past it
    fn remove_inflight_and_advance_head(&mut self) {
        if let Some(id) = self.current_withdraw_inflight.take() {
            let _ = self.pending_withdrawals.remove(&id);
            self.next_withdraw_to_execute = id.saturating_add(1);
            Event::WithdrawDequeued { index: id.into() }.emit();
        }
    }

    // Keep the head pending but clear in-flight so it can be retried later
    fn park_inflight_head_for_retry(&mut self) {
        if let Some(current_withdraw_inflight) = self.current_withdraw_inflight {
            Event::WithdrawalParked {
                id: current_withdraw_inflight.into(),
            }
            .emit();
        }
        self.current_withdraw_inflight = None;
    }
}

/* ----- Views ----- */
#[near]
impl Contract {
    /// # Panics
    /// - If the owner is not set
    /// - If the curator is not set
    /// - If the guardian is not set
    #[allow(clippy::expect_used, reason = "No side effects")]
    pub fn get_configuration(&self) -> VaultConfiguration {
        let meta = self.get_metadata();
        VaultConfiguration {
            owner: self.own_get_owner().unwrap_or_else(|| {
                templar_common::panic_with_message("Owner not set in get_configuration")
            }),
            curator: Self::with_members_of(&Role::Curator, |members| {
                require!(
                    members.len() == 1,
                    "Invariant violation: Cannot have more than one Curator"
                );
                members
                    .iter()
                    .next()
                    .expect("Curator not set in get_configuration")
                    .clone()
            }),
            guardian: Self::with_members_of(&Role::Guardian, |members| {
                require!(
                    members.len() == 1,
                    "Invariant violation: Cannot have more than one Guardian"
                );
                members
                    .iter()
                    .next()
                    .expect("Guardian not set in get_configuration")
                    .clone()
            }),
            underlying_token: self.underlying_asset.clone(),
            initial_timelock_ns: self.governance_timelocks.timelock_config_ns.into(),
            fee_recipient: self.fee_recipient.clone(),
            skim_recipient: self.skim_recipient.clone(),
            name: meta.name,
            symbol: meta.symbol,
            decimals: NonZeroU8::new(meta.decimals).expect("Decimals must be non-zero"),
            mode: self.mode.clone(),
            restrictions: self.gate.restrictions.clone(),
        }
    }

    /// Returns total assets under management = idle balance + sum of market principals.
    pub fn get_total_assets(&self) -> U128 {
        self.aum.get_total_assets(self)
    }

    pub fn get_idle_balance(&self) -> U128 {
        self.idle_balance.into()
    }

    pub fn get_total_supply(&self) -> U128 {
        U128(self.total_supply())
    }

    /// Returns a best-effort estimate of the maximum additional amount that can be deposited
    /// across all markets given current caps and the current `supply_queue`.
    ///
    /// This does not reserve capacity and may become stale immediately after it is read.
    pub fn get_max_deposit(&self) -> U128 {
        let total = self
            .supply_queue
            .iter()
            .fold(0u128, |acc, m| match self.markets.get(m) {
                Some(rec) if rec.cfg.cap.0 > 0 => acc + rec.cfg.cap.0.saturating_sub(rec.principal),
                _ => acc,
            });
        U128(total)
    }

    /// Returns a best-effort estimate of the maximum additional amount that can be deposited
    /// into any single market in the current `supply_queue`, given current caps.
    ///
    /// This is intended for UIs that want to route deposits in a way that is consistent with
    /// the vault's `supply_queue`. It does not reserve capacity and may become stale
    /// immediately after it is read.
    pub fn get_max_single_market_deposit(&self) -> U128 {
        let max_room = self
            .supply_queue
            .iter()
            .fold(0u128, |acc, m| match self.markets.get(m) {
                Some(rec) if rec.cfg.cap.0 > 0 => {
                    acc.max(rec.cfg.cap.0.saturating_sub(rec.principal))
                }
                _ => acc,
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

    pub fn has_pending_market_withdrawal(&self) -> bool {
        !self.market_execution_lock.is_empty()
    }

    pub fn get_current_withdraw_request_id(&self) -> Option<U64> {
        self.current_withdraw_inflight.map(Into::into)
    }
}

/* ----- Private Helpers ----- */
impl Contract {
    // Principal (vault-supplied) units currently recorded for a market
    fn principal_of(&self, market: &AccountId) -> u128 {
        self.markets.get(market).map_or(0, |r| r.principal)
    }

    fn cap_of(&self, market: &AccountId) -> u128 {
        self.markets.get(market).map_or(0, |r| r.cfg.cap.0)
    }

    // Remaining room until cap for a market
    fn room_of(&self, market: &AccountId) -> u128 {
        self.cap_of(market)
            .saturating_sub(self.principal_of(market))
    }

    /// Enqueue a vault-level pending withdrawal request.
    /// At this point the escrow shares are already taken.
    fn enqueue_pending_withdrawal(
        &mut self,
        owner: &AccountId,
        receiver: &AccountId,
        escrow_shares: u128,
        expected_assets: u128,
    ) {
        let id = self.next_withdraw_id;
        self.next_withdraw_id = self.next_withdraw_id.saturating_add(1);
        let requested_at = env::block_timestamp();

        self.pending_withdrawals.insert(
            id,
            PendingWithdrawal {
                owner: owner.clone(),
                receiver: receiver.clone(),
                escrow_shares,
                expected_assets,
                requested_at,
            },
        );

        Event::WithdrawalQueued {
            id: id.into(),
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: U128(escrow_shares),
            expected_assets: U128(expected_assets),
            requested_at: requested_at.into(),
        }
        .emit();
    }

    fn create_withdraw_request_for_market(
        &mut self,
        op_id: u64,
        index: u32,
        remaining: u128,
        receiver: &AccountId,
        collected: u128,
        owner: &AccountId,
        escrow_shares: u128,
        market: AccountId,
    ) -> PromiseOrValue<()> {
        let have = self.principal_of(&market);
        let to_request = have.min(remaining);
        if to_request == 0 {
            self.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                index: index + 1,
                remaining,
                receiver: receiver.clone(),
                collected,
                owner: owner.clone(),
                escrow_shares,
            });
            env::log_str(&format!(
                "Skipping withdrawal for market {market} (have {have}, remaining {remaining})"
            ));
            return self.step_withdraw();
        }
        PromiseOrValue::Promise(
            ext_market::ext(market.clone())
                .with_static_gas(CREATE_WITHDRAW_REQ_GAS)
                .create_supply_withdrawal_request(BorrowAssetAmount::from(U128(to_request)))
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(AFTER_CREATE_WITHDRAW_REQ_GAS)
                        .withdraw_01_handle_create_request(op_id, index, U128(to_request)),
                ),
        )
    }

    /// Computes fee-aware effective totals for conversions, mimicking `MetaMorpho`:
    /// - Include fee shares that would be minted if fees accrued now.
    /// - Apply virtual offsets: +`virtual_shares` to supply and +`virtual_assets` to assets.
    fn effective_totals_fee_aware(&self) -> (u128, u128) {
        let cur = self.get_total_assets().0;
        let ts = self.total_supply();
        let (new_total_supply, new_total_assets) = Self::compute_effective_totals(
            cur.into(),
            self.last_total_assets.into(),
            self.performance_fee,
            ts.into(),
            self.virtual_shares.into(),
            self.virtual_assets.into(),
        );
        (new_total_supply.into(), new_total_assets.into())
    }

    // Pure helper to compute how many escrowed shares to burn on partial payout
    fn compute_burn_shares(escrow_shares: u128, collected: u128, requested_total: u128) -> u128 {
        mul_div_floor(
            escrow_shares.into(),
            collected.into(),
            requested_total.max(1).into(),
        )
        .into()
    }

    pub(crate) fn compute_effective_totals(
        cur_assets: Number,
        last_total_assets: Number,
        performance_fee: wad::Wad,
        total_supply: Number,
        virtual_shares: Number,
        virtual_assets: Number,
    ) -> (Number, Number) {
        let fee_shares =
            compute_fee_shares(cur_assets, last_total_assets, performance_fee, total_supply);
        let new_total_supply = total_supply
            .saturating_add(fee_shares)
            .saturating_add(virtual_shares);
        let new_total_assets = cur_assets.saturating_add(virtual_assets);
        (new_total_supply, new_total_assets)
    }

    pub(crate) fn clamp_allocation_total(&self, requested: Option<u128>) -> u128 {
        let requested = requested.unwrap_or(self.idle_balance);
        let max_room = self.get_max_deposit().0;
        requested.min(self.idle_balance).min(max_room)
    }

    pub(crate) fn compute_escrow_settlement(
        escrow_shares: u128,
        burn_shares: u128,
    ) -> EscrowSettlement {
        let to_burn = burn_shares.min(escrow_shares);
        let refund = escrow_shares.saturating_sub(to_burn);
        EscrowSettlement { to_burn, refund }
    }

    pub fn internal_accrue_fee(&mut self) {
        // Invariant: Fees are minted only when total_assets() > last_total_assets (no fees on losses/flat).
        let cur = self.get_total_assets().0;
        let fee_shares = compute_fee_shares(
            cur.into(),
            self.last_total_assets.into(),
            self.performance_fee,
            self.total_supply().into(),
        );
        if fee_shares > Number::zero() {
            let minted: u128 = fee_shares.into();
            let recipient = self.fee_recipient.clone();
            let _ = self
                .mint(&Nep141Mint::new(minted, &recipient))
                .inspect_err(|e| env::log_str(&format!("Failed to mint {e}")));
            Event::PerformanceFeeAccrued {
                recipient,
                shares: U128(minted),
            }
            .emit();
        }
        self.last_total_assets = cur;
    }

    /* ----- Auth ----- */
    fn assert_guardian_or_owner() {
        let p = env::predecessor_account_id();

        if !Self::has_role(&p, &Role::Guardian) {
            Self::require_owner();
        }
    }

    fn assert_curator_or_owner() {
        let p = env::predecessor_account_id();
        if !Self::has_role(&p, &Role::Curator) {
            Self::require_owner();
        }
    }

    fn assert_allocator() {
        let p = env::predecessor_account_id();
        if !Self::has_role(&p, &Role::Allocator) && !Self::has_role(&p, &Role::Curator) {
            Self::require_owner();
        }
    }

    /* ----- Internal: op orchestration ----- */
    fn ensure_idle(&self) {
        // Invariant: Only one op in flight; ensure_idle() guards all mutating ops.
        if !matches!(self.op_state, OpState::Idle) {
            templar_common::panic_with_message(&format!(
                "Invariant: Only one op in flight; current op_state = {:?}",
                self.op_state
            ));
        }
    }

    fn start_allocation(&mut self, amount: u128, plan: AllocationPlan) -> PromiseOrValue<()> {
        if amount == 0 {
            return PromiseOrValue::Value(());
        }
        self.ensure_idle();

        require!(
            amount <= self.idle_balance,
            "Policy violation: reserve amount must be <= idle_balance"
        );
        self.update_idle_balance(IdleBalanceDelta::Decrease(amount.into()));

        let op_id = self.next_op_id;
        self.next_op_id += 1;
        self.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: amount,
            plan,
        });
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
        market: &AccountId,
        amount: u128,
        op_id: u64,
        index: u32,
        remaining_before: u128,
    ) -> Promise {
        self::require_at_least(AFTER_SUPPLY_1_CHECK_GAS.saturating_add(GAS_FOR_FT_TRANSFER_CALL));
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
                    .with_static_gas(AFTER_SUPPLY_1_CHECK_GAS)
                    .supply_01_handle_transfer(
                        market.clone(),
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
        if let Some((market, amount)) = plan.get(idx) {
            let market_id = market.clone();

            let room = self.room_of(&market_id);
            let to_supply = room.min(*amount);

            Event::AllocationStepPlanned {
                op_id: op_id.into(),
                index,
                market: market_id.clone(),
                target: U128(*amount),
                room: U128(room),
                to_supply: U128(to_supply),
                remaining_before: U128(remaining),
                    planned: true,
                }
                .emit();

                if to_supply == 0 {
                    Event::AllocationStepSkipped {
                        op_id: op_id.into(),
                        index,
                        market: market_id.clone(),
                        reason: if room == 0 {
                            "no-room".to_string()
                        } else {
                            "zero-target".to_string()
                        },
                        remaining: U128(remaining),
                    }
                    .emit();

                    self.op_state = OpState::Allocating(AllocatingState {
                    op_id,
                    index: index + 1,
                    remaining,
                    plan: plan.into_iter().filter(|m| m.0 != market_id).collect(),
                });
                return self.step_allocation();
            }

                PromiseOrValue::Promise(
                    self.supply_and_then(&market_id, to_supply, op_id, index, remaining),
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
        route: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        if amount == 0 {
            return self.stop_and_exit(Some(&Error::ZeroAmount));
        }
        self.ensure_idle();
        let op_id = self.next_op_id;
        self.next_op_id += 1;

        // Policy: Idle-first reservation does not mutate idle_balance until payout succeeds.
        let (remaining, collected_from_idle) = self.idle_delta(amount);

        self.market_execution_lock.clear();
        self.withdraw_route = route;

        self.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            index: Default::default(),
            remaining,
            receiver: receiver.clone(),
            collected: collected_from_idle,
            owner: owner.clone(),
            escrow_shares,
        });
        self.step_withdraw()
    }

    fn step_withdraw(&mut self) -> PromiseOrValue<()> {
        let OpState::Withdrawing(WithdrawingState {
            op_id,
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
            // Already fully covered by idle => payout
            self.pay(
                op_id,
                &receiver,
                collected,
                &owner,
                escrow_shares,
                escrow_shares,
            );
        }
        if let Some(market) = self.withdraw_route.get(index as usize) {
            self.create_withdraw_request_for_market(
                op_id,
                index,
                remaining,
                &receiver,
                collected,
                &owner,
                escrow_shares,
                market.clone(),
            )
        } else {
            let requested = collected.saturating_add(remaining);
            let burn_shares = Self::compute_burn_shares(escrow_shares, collected, requested);

            self.pay_or_else(
                op_id,
                &receiver,
                collected,
                &owner,
                escrow_shares,
                burn_shares,
                |self_| {
                    self_.withdraw_route.clear();
                    self_.op_state = OpState::Idle;
                    self_.park_inflight_head_for_retry();
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
        receiver: &AccountId,
        amount: u128,
        owner: &AccountId,
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
        receiver: &AccountId,
        amount: u128,
        owner: &AccountId,
        escrow_shares: u128,
        burn_shares: u128,
    ) -> PromiseOrValue<()> {
        self.op_state = OpState::Payout(PayoutState {
            op_id,
            receiver: receiver.clone(),
            amount,
            owner: owner.clone(),
            escrow_shares,
            burn_shares,
        });
        require!(self.idle_balance >= amount, "idle underflow in payout");
        self.update_idle_balance(IdleBalanceDelta::Decrease(amount.into()));

        PromiseOrValue::Promise(
            self.underlying_asset
                .transfer(receiver.clone(), U128(amount).into())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(AFTER_SEND_TO_USER_GAS)
                        .payment_01_reconcile_idle_or_refund(op_id, receiver.clone(), U128(amount)),
                ),
        )
    }

    fn idle_delta(&mut self, amount: u128) -> (u128, u128) {
        let used_idle = self.idle_balance.min(amount);
        let remaining = amount.saturating_sub(used_idle);
        let collected = used_idle;
        (remaining, collected)
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
