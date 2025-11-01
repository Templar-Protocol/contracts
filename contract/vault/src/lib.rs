#![allow(clippy::needless_pass_by_value)]

use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU8,
};

use crate::{
    aum::AUM,
    storage_management::{
        require_attached_at_least, require_attached_for_pending_withdrawal,
        storage_bytes_for_queue_account_id, yocto_for_bytes, yocto_for_new_market,
        yocto_for_pending_cap,
    },
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, serde_json,
    store::{IterableMap, Vector},
    AccountId, BorshStorageKey, IntoStorageKey, PanicOnDefault, Promise, PromiseOrValue,
};
use near_sdk_contract_tools::{
    ft::{
        nep141::GAS_FOR_FT_TRANSFER_CALL, nep145::Nep145ForceUnregister, ContractMetadata,
        FungibleToken, Nep141Controller, Nep141Mint, Nep145 as _, Nep148Controller,
    },
    Owner, Rbac,
};
use near_sdk_contract_tools::{owner::Owner, rbac};
use near_sdk_contract_tools::{owner::OwnerExternal, rbac::Rbac};
use templar_common::{
    asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset},
    vault::{
        ext_self, require_at_least, AllocatingState, AllocationMode, AllocationPlan,
        AllocationWeights, Error, Event, MarketConfiguration, OpState, PayoutState, PendingValue,
        PendingWithdrawal, TimestampNs, VaultConfiguration, WithdrawingState,
        AFTER_CREATE_WITHDRAW_REQ_GAS, AFTER_EXECUTE_NEXT_WITHDRAW_GAS, AFTER_SEND_TO_USER_GAS,
        AFTER_SUPPLY_1_CHECK_GAS, ALLOCATE_GAS, CREATE_WITHDRAW_REQ_GAS, EXECUTE_WITHDRAW_GAS,
        MAX_QUEUE_LEN, MAX_TIMELOCK_NS, MIN_TIMELOCK_NS, WITHDRAW_GAS,
    },
};
pub use wad::*;

pub mod aum;
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
    Config,
    PendingCaps,
    SupplyQueue,
    WithdrawQueue,
    MarketSupply,
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
    /// Has no authority to change caps or queues on its own.
    Guardian,
    /// Operational role for queue maintenance.
    /// May set the supply/withdraw queues while the vault is Idle; cannot modify caps/timelocks/guardian.
    Allocator,
}

#[derive(PanicOnDefault, FungibleToken, Owner, Rbac)]
#[fungible_token(force_unregister_hook = "Self")]
#[rbac(roles = "Role", crate = "crate")]
#[near(contract_state)]
/// Vault contract that issues shares over an underlying fungible asset and allocates liquidity
/// across configured markets. Implements 4626-like deposit/withdraw semantics.
///
/// What this contract does (high-level mental model)
/// - Issues a share token (NEP-141) that represents a vault over an underlying NEP-141 “BorrowAsset”.
/// - Allocates deposits across “markets” (external contracts) via a supply queue, and withdraws via a withdraw queue.
/// - Governance uses Owner + RBAC (Curator/Guardian/Allocator) with a timelock for certain changes.
/// - Withdraw flow escrows shares, builds market-side withdrawal requests, then pays out and burns proportional escrow.
/// - Performance fees accrue by minting fee shares based on increases in total assets.
/// Critical invariants the code intends to keep
/// - Assets accounting is correct: total_assets = idle_balance + sum(all principals in markets).
/// - Withdraw queue contains every market that either is enabled or still holds principal (until that principal is zero).
/// - Only one op in flight (op_state); mutating ops require Idle.
/// - Governance changes obey timelocks; Guardian may revoke pending changes.
///
/// Note: RBAC storage (role membership) is paid by the contract; callers are not charged deposits for RBAC changes.
pub struct Contract {
    /// The underlying asset that the vault manages
    underlying_asset: FungibleAsset<BorrowAsset>,

    /// The process in which the vault calculates its assets under management (AUM)
    aum: AUM,

    /// The mode in which the allocator will operate
    mode: AllocationMode,
    plan: Option<AllocationPlan>,

    /// Performance fee
    performance_fee: wad::Wad,
    fee_recipient: AccountId,
    skim_recipient: AccountId,
    /// Last recorded total assets (for fee accrual)
    last_total_assets: u128,

    // Virtual offsets used only in conversions/previews to harden edge cases
    virtual_shares: u128,
    virtual_assets: u128,

    // FIXME: think about merging markets, pending cap and market_supply
    /// configuration per market (market ID -> MarketConfig)
    markets: IterableMap<AccountId, MarketConfiguration>,
    /// Any pending change to the vault's cap, TODO: u256
    pending_cap: IterableMap<AccountId, PendingValue<u128>>,
    /// vault's supplied principal per market (borrow-asset units)
    market_supply: IterableMap<AccountId, u128>,

    /// Any pending change to the vault's timelock
    pending_timelock: Option<PendingValue<TimestampNs>>,
    /// Any pending change to the vault's guardian
    pending_guardian: Option<PendingValue<AccountId>>,
    /// Current timelock duration for governance actions (ns)
    timelock_ns: TimestampNs,

    /// Ordered list of market IDs for deposit allocation
    supply_queue: Vector<AccountId>,
    /// Ordered list of market IDs for withdrawal prioritytr
    withdraw_queue: Vector<AccountId>,
    current_withdraw_inflight: Option<u64>, // id of the pending withdrawal being executed, if any

    /// underlying held by vault
    idle_balance: u128,
    op_state: OpState,
    next_op_id: u64,

    /// Pending withdrawals queue (vault-level, FIFO by id)
    pending_withdrawals: IterableMap<u64, PendingWithdrawal>,
    next_withdraw_id: u64,
    next_withdraw_to_execute: u64,

    // if true, only create requests during build; executor will run them
    defer_market_execute: bool,
    // indices of markets with created requests (per withdrawing op)
    pending_market_exec: Vec<u32>,
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
            mode,
        } = configuration;

        require!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&initial_timelock_ns.0),
            "timelock bounds"
        );

        let prefix = b"v";
        // TODO: this is copied from market, make a helper
        let prefix = prefix.into_storage_key();
        macro_rules! key {
            ($key: ident) => {
                [
                    prefix.as_slice(),
                    StorageKey::$key.into_storage_key().as_slice(),
                ]
                .concat()
            };
        }

        let mut contract = Self {
            underlying_asset: underlying_token,
            aum: AUM::GovernanceAbandonment,
            timelock_ns: initial_timelock_ns.0,
            performance_fee: Default::default(),
            fee_recipient,
            skim_recipient,
            markets: IterableMap::new(key!(Config)),
            pending_cap: IterableMap::new(key!(PendingCaps)),
            pending_timelock: None,
            pending_guardian: None,
            supply_queue: Vector::new(key!(SupplyQueue)),
            withdraw_queue: Vector::new(key!(WithdrawQueue)),
            market_supply: IterableMap::new(key!(MarketSupply)),
            last_total_assets: 0,
            virtual_shares: 1,
            virtual_assets: 1,
            idle_balance: 0,
            op_state: OpState::Idle,
            next_op_id: 1,
            mode,
            plan: None,
            current_withdraw_inflight: None,

            // Pending withdrawals init
            pending_withdrawals: IterableMap::new(key!(PendingWithdrawals)),
            next_withdraw_id: 0,
            next_withdraw_to_execute: 0,

            // Deferred market execution
            defer_market_execute: true, // default to “stop executing automatically” per request
            pending_market_exec: Vec::new(),
        };
        contract.set_metadata(&ContractMetadata::new(name, symbol, decimals.into()));
        Owner::init(&mut contract, &owner);
        Rbac::add_role(&mut contract, &curator, &Role::Curator);
        Rbac::add_role(&mut contract, &curator, &Role::Allocator);
        Rbac::add_role(&mut contract, &guardian, &Role::Guardian);

        contract
    }

    /// Sets the Curator account. Also grants/removes the Allocator role accordingly.
    pub fn set_curator(&mut self, account: AccountId) {
        Self::require_owner();
        Self::with_members_of_mut(&Role::Curator, |members| {
            require!(
                members.len() < 2,
                "Invariant violation: Cannot have more than one Curator"
            );
            require!(
                !members.contains(&account),
                "Curator already set to this account"
            );
            members.iter().for_each(|m| {
                self.set_is_allocator(m, false);
            });
            members.clear();
        });
        Self::add_role(self, &account, &Role::Curator);
        Event::CuratorSet {
            account: account.clone(),
        }
        .emit();
        self.set_is_allocator(account, true);
    }

    /// Grants or revokes the Allocator role for `account`.
    pub fn set_is_allocator(&mut self, account: AccountId, allowed: bool) {
        Self::require_owner();
        if allowed {
            Self::add_role(self, &account, &Role::Allocator);
        } else {
            self.remove_role(&account, &Role::Allocator);
        }
        Event::AllocatorRoleSet { account, allowed }.emit();
    }

    /// Proposes a new Guardian. If a Guardian already exists, starts a timelock; otherwise sets immediately.
    pub fn submit_guardian(&mut self, new_g: AccountId) {
        Self::require_owner();
        let mut guardian_occupied = false;

        Self::with_members_of(&Role::Guardian, |members| {
            require!(
                members.len() < 2,
                "Invariant violation: Cannot have more than one Guardian"
            );
            require!(!members.contains(&new_g), "Already set to this address");
            guardian_occupied = !members.is_empty();
        });
        require!(
            self.pending_guardian.is_none(),
            "Guardian change already pending"
        );
        if guardian_occupied {
            let valid_at_ns = env::block_timestamp() + self.timelock_ns;
            self.pending_guardian = Some(PendingValue {
                value: new_g,
                valid_at_ns,
            });
        } else {
            Self::add_role(self, &new_g, &Role::Guardian);
            Event::GuardianSet {
                account: new_g.clone(),
            }
            .emit();
        }
    }

    /// Accepts the pending Guardian change after the timelock has elapsed.
    pub fn accept_guardian(&mut self) {
        Self::require_owner();

        let p = self.pending_guardian.clone();

        if let Some(p) = &p {
            p.verify();
            Self::with_members_of_mut(&Role::Guardian, |members| {
                members.clear();
                members.insert(&p.value);
            });
            Event::GuardianSet {
                account: p.value.clone(),
            }
            .emit();
            self.pending_guardian = None;
        }
    }

    /// Revokes any pending Guardian change.
    pub fn revoke_pending_guardian(&mut self) {
        Self::assert_guardian_or_owner();
        self.pending_guardian = None;
    }

    /// Sets the recipient account for skimmed tokens.
    pub fn set_skim_recipient(&mut self, account: AccountId) {
        Self::require_owner();
        require!(
            account != self.skim_recipient,
            "Already set to this address"
        );
        self.skim_recipient = account.clone();
        Event::SkimRecipientSet {
            account: account.clone(),
        }
        .emit();
    }

    /// Sets the performance fee recipient. Accrues pending fees with the current recipient first.
    #[payable]
    pub fn set_fee_recipient(&mut self, account: AccountId) {
        Self::require_owner();
        require!(account != self.fee_recipient, "Already set to this address");

        if self.performance_fee != wad::Wad::zero() {
            // Accrue any pending fees to current recipient before changing (so current recipient gets up to now)
            self.internal_accrue_fee();
        }
        Event::FeeRecipientSet {
            account: account.clone(),
        }
        .emit();
        self.storage_deposit(Some(account.clone()), Some(true));

        self.fee_recipient = account;
    }

    /// Sets the performance fee as a WAD fraction (1e24 = 100%). Accrues fees at the old rate first.
    pub fn set_performance_fee(&mut self, fee: Wad) {
        Self::require_owner();

        require!(fee != self.performance_fee, "Fee already set to this value");
        require!(fee <= Wad::from(MAX_FEE_WAD), "fee too high");

        // Accrue any pending fees with old rate before changing
        self.internal_accrue_fee();
        self.performance_fee = fee;
        Event::PerformanceFeeSet {
            fee: U128(u128::from(fee)),
        }
        .emit();
    }

    /* ----- Timelocks / Pending ----- */
    /// Proposes a new governance timelock in nanoseconds.
    /// If increasing, applies immediately; if decreasing, starts a timelock equal to the current duration.
    pub fn submit_timelock(&mut self, new_timelock_ns: U64) {
        Self::require_owner();
        let tl = &new_timelock_ns.0;

        require!(tl != &self.timelock_ns, "Already set to this value");
        require!(
            self.pending_timelock.is_none(),
            "Timelock change already pending"
        );
        require!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(tl),
            "Timelock out of bounds"
        );
        if tl > &self.timelock_ns {
            self.timelock_ns = *tl;
            Event::TimelockSet {
                seconds: new_timelock_ns,
            }
            .emit();
        } else {
            let valid_at_ns = env::block_timestamp() + self.timelock_ns;
            self.pending_timelock = Some(PendingValue {
                value: *tl,
                valid_at_ns,
            });
            Event::TimelockChangeSubmitted {
                new_ns: new_timelock_ns,
                valid_at_ns: valid_at_ns.into(),
            }
            .emit();
        }
    }

    /// Accepts a pending timelock change after it becomes valid.
    pub fn accept_timelock(&mut self) {
        Self::require_owner();
        if let Some(p) = &self.pending_timelock {
            p.verify();

            self.timelock_ns = p.value;
            Event::TimelockSet {
                seconds: p.value.into(),
            }
            .emit();
            self.pending_timelock = None;
        } else {
            env::panic_str("No pending timelock change");
        }
    }

    /// Revokes any pending timelock change.
    pub fn revoke_pending_timelock(&mut self) {
        Self::assert_guardian_or_owner();
        self.pending_timelock = None;
        Event::PendingTimelockRevoked {}.emit();
    }

    /* ----- Market config / queues ----- */
    /// Submits a change to a market's supply cap.
    /// Decreases apply immediately; increases are subject to the governance timelock.
    #[payable]
    pub fn submit_cap(&mut self, market: AccountId, new_cap: U128) {
        Self::assert_curator_or_owner();
        self.ensure_idle();

        let mut required_deposit: u128 = 0;
        if self.markets.get(&market).is_none() {
            required_deposit = required_deposit.saturating_add(yocto_for_new_market());
        }
        let current_cap = self.markets.get(&market).map_or(0, |c| c.cap.0);
        if new_cap.0 > current_cap {
            required_deposit = required_deposit.saturating_add(yocto_for_pending_cap());
        }
        require_attached_at_least(required_deposit, "submit_cap");

        require!(
            self.pending_cap.get(&market).is_none(),
            "Policy violation: A cap change is already pending for this market"
        );

        let config = match self.markets.get_mut(&market) {
            None => {
                self.markets
                    .insert(market.clone(), MarketConfiguration::default());
                Event::MarketCreated {
                    market: market.clone(),
                }
                .emit();
                // Pre-allocate a market_supply record (principal=0) so allocations don't create storage later
                self.market_supply.insert(market.clone(), 0);
                self.cfg_mut(&market)
            }
            Some(config) => config,
        };

        require!(
            config.removable_at == 0,
            "Market removal pending, cannot change cap"
        );
        require!(new_cap != config.cap, "New cap is same as current");

        if new_cap < config.cap {
            // If lowering the cap, we can apply the delta immediately
            config.cap = new_cap;
        } else {
            let valid_at_ns = env::block_timestamp() + self.timelock_ns;
            self.pending_cap.insert(
                market.clone(),
                PendingValue {
                    value: new_cap.0,
                    valid_at_ns,
                },
            );
            Event::SupplyCapRaiseSubmitted {
                market: market.clone(),
                new_cap,
                valid_at_ns,
            }
            .emit();
        }
    }

    /// Accepts a pending cap increase for `market` once the timelock has elapsed.
    #[payable]
    pub fn accept_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        self.ensure_idle();

        let pending_value = match self.pending_cap.get(&market) {
            Some(p) => {
                p.verify();
                p.value
            }
            None => env::panic_str("No pending cap change for this market"),
        };

        let was_enabled = self.cfg(&market).enabled;
        let in_queue = self.in_withdraw_queue(&market);
        let before_principal = self.principal_of(&market);

        let cfg = self.cfg_mut(&market);
        cfg.cap = pending_value.into();
        if pending_value > 0 {
            if !cfg.enabled {
                cfg.enabled = true;
            }
            cfg.removable_at = 0;
        }

        // If we just enabled the market, ensure it's in the withdraw queue
        if pending_value > 0 && !was_enabled {
            Event::MarketEnabled {
                market: market.clone(),
            }
            .emit();

            if in_queue {
                Event::MarketAlreadyInWithdrawQueue {
                    market: market.clone(),
                }
                .emit();
            } else {
                let _ = require_attached_at_least(
                    yocto_for_bytes(storage_bytes_for_queue_account_id()),
                    "withdraw queue entry",
                );
                self.add_market_to_withdraw_queue(&market, before_principal);
            }
        }

        Event::SupplyCapSet {
            market: market.clone(),
            new_cap: U128(pending_value),
        }
        .emit();

        // Finally, clear the pending cap record
        self.pending_cap.remove(&market);
    }

    /// Revokes any pending cap change for `market`.
    pub fn revoke_pending_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        if self.pending_cap.get(&market).is_some() {
            self.pending_cap.remove(&market);
            Event::SupplyCapRaiseRevoked {
                market: market.clone(),
            }
            .emit();
        }
    }

    /// To remove a market entirely, the curator:
    ///- first sets its cap to 0 (disabling new deposits)
    ///- then calls submit_market_removal.
    /// > This starts a timelock (using the vault’s timelock)
    /// - after which the market can be removed from the withdraw_queue (assuming any funds have been withdrawn)
    /// Begins the process to remove `market` from the withdraw queue.
    /// Requires cap == 0 and no pending cap changes; starts a timelock.
    pub fn submit_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        let cfg = self
            .markets
            .get_mut(&market)
            .unwrap_or_else(|| env::panic_str(&format!("Unknown market: {market}")));
        require!(
            cfg.removable_at == 0,
            "Removal already pending for this market"
        );
        require!(
            cfg.cap.0 == 0,
            "Cannot remove market with non-zero cap (disable deposits first)"
        );
        require!(cfg.enabled, "Market not enabled or already removed");
        require!(
            self.pending_cap.get(&market).is_none(),
            "Cap change pending for this market"
        );
        cfg.removable_at = env::block_timestamp() + self.timelock_ns;
        Event::MarketRemovalSubmitted {
            market: market.clone(),
            removable_at: cfg.removable_at.into(),
        }
        .emit();
    }
    /// Revokes a pending market removal for `market`.
    pub fn revoke_pending_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        if let Some(cfg) = self.markets.get_mut(&market) {
            cfg.removable_at = 0;
        }
        Event::MarketRemovalRevoked { market }.emit();
    }

    /// Sets the ordered supply (allocation) queue.
    /// Rejects duplicates and markets without a positive cap. Requires the vault to be idle.
    #[payable]
    pub fn set_supply_queue(&mut self, markets: Vec<AccountId>) {
        Self::assert_allocator();
        self.ensure_idle();
        require!(markets.len() <= MAX_QUEUE_LEN, "too long");

        // Invariant: supply_queue has no duplicates; allocation order remains meaningful
        let mut seen = HashSet::new();
        for m in &markets {
            if !seen.insert(m.clone()) {
                env::panic_str(&format!("Duplicate market {m}"));
            }
        }
        // Validate all markets are authorized (cap > 0) before charging storage
        for m in &markets {
            let cap = self.markets.get(m).map_or(0, |c| c.cap.into());
            require!(cap > 0, "unauthorized market");
        }

        // Compute and require storage for additions (no refunds for removals in this pass)
        let current: HashSet<AccountId> = self.supply_queue.iter().cloned().collect();
        let required_yocto = storage_management::yocto_for_queue_additions(&current, &markets);
        require_attached_at_least(required_yocto, "supply queue update");

        self.supply_queue.clear();

        for m in &markets {
            self.supply_queue.push(m.clone());
        }
    }

    /// For each removed market, we enforce the conditions:
    /// Cap is 0 (no new deposits).
    ///
    /// No pending cap change.
    ///
    /// If the vault still has a supply in that market (vault_shares_in_market > 0), the market must have had submit_market_removal called (removable_at set) and the timelock must have passed.
    /// Sets the ordered withdraw queue.
    /// Enforces safety invariants and the policy that all enabled/holding markets must be present.
    #[payable]
    pub fn set_withdraw_queue(&mut self, queue: Vec<AccountId>) {
        Self::assert_allocator();
        self.ensure_idle();
        require!(
            queue.len() <= MAX_QUEUE_LEN,
            "Withdraw queue length exceeds max"
        );

        let mut seen = HashSet::new();
        for id in &queue {
            if !seen.insert(id.clone()) {
                env::panic_str(&format!("Duplicate market {id}"));
            }
        }

        // Snapshot current withdraw queue into a set for membership checks
        let current: HashSet<AccountId> = self.withdraw_queue.iter().cloned().collect();

        for id in &queue {
            require!(
                self.markets.get(id).is_some(),
                "Policy violation: Unknown market in new queue"
            );
        }

        for (id, cfg) in self.markets.iter() {
            let has_supply = *self.market_supply.get(id).unwrap_or(&0) > 0;
            if (cfg.enabled || has_supply) && !seen.contains(id) {
                if current.contains(id) {
                    // Omission is allowed only when removing an existing queued market AND all safety preconditions hold.
                    require!(
                        cfg.cap.0 == 0,
                        "Policy violation: Cannot remove market with non-zero cap"
                    );
                    require!(
                        self.pending_cap.get(id).is_none(),
                        "Policy violation: Cannot remove market with pending cap change"
                    );
                    self.aum.policy_removal(cfg, &has_supply);
                } else {
                    // Not in current queue: must be included if enabled or holding.
                    env::panic_str(
                        &format!(
                            "Invariant violation: Withdraw queue must include all enabled or holding markets; missing {id}"
                        ),
                    );
                }
            }
        }

        let required_yocto = storage_management::yocto_for_queue_additions(&current, &queue);
        let _ = require_attached_at_least(required_yocto, "withdraw queue update");

        self.withdraw_queue.clear();
        for id in &queue {
            self.withdraw_queue.push(id.clone());
        }
        Event::WithdrawQueueUpdated {
            markets: queue.clone(),
        }
        .emit();
    }

    /* ----- Withdraw / Redeem ----- */
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

        require!(shares > 0, "Invalid shares");

        let _ = require_attached_for_pending_withdrawal();

        // Move shares into escrow
        #[allow(clippy::expect_used, reason = "No side effects")]
        self.transfer_unchecked(&sender, &env::current_account_id(), shares)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        self.internal_accrue_fee();

        Event::RedeemRequested {
            shares: U128(shares),
            estimated_assets: U128(assets),
        }
        .emit();

        self.enqueue_pending_withdrawal(&sender, &receiver, shares, assets);
        PromiseOrValue::Value(())
    }

    /// Executes the next pending withdrawal request, if any, using the existing withdraw pipeline.
    /// This defers creating market-side withdrawal requests until explicitly invoked.
    pub fn execute_next_withdrawal_request(&mut self) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        self.ensure_idle();
        Self::assert_allocator();

        if self.current_withdraw_inflight.is_some() {
            env::panic_str("A pending withdrawal is already in-flight");
        }

        if let Some(id) = self.peek_next_pending_withdrawal_id() {
            let pending = self
                .pending_withdrawals
                .get(&id)
                .unwrap_or_else(|| env::panic_str("pending vanished unexpectedly"));
            let owner = pending.owner.clone();
            let receiver = pending.receiver.clone();
            self.current_withdraw_inflight = Some(id);
            env::log_str(&format!("WithdrawalExecutionStarted id={id}"));
            return self.start_withdraw(
                pending.expected_assets,
                &receiver,
                &owner,
                pending.escrow_shares,
            );
        }

        PromiseOrValue::Value(())
    }

    /// Executes one created market withdrawal request in the current Withdrawing op.
    pub fn allocator_execute_next_market_withdrawal(&mut self, op_id: u64) -> PromiseOrValue<()> {
        require_at_least(EXECUTE_WITHDRAW_GAS);
        Self::assert_allocator();

        // Must be in Withdrawing context for the provided op_id
        let _ctx = match self.ctx_withdrawing(op_id) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        // Ensure we have a created request to execute
        let market_index = match self.pending_market_exec.first().copied() {
            Some(idx) => idx,
            None => {
                env::panic_str("No pending market withdrawal request to execute");
            }
        };

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        PromiseOrValue::Promise(
            templar_common::market::ext_market::ext(market.clone())
                .with_static_gas(templar_common::vault::EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS)
                .with_unused_gas_weight(0)
                .execute_next_supply_withdrawal_request()
                .then(
                    ext_self::ext(env::current_account_id())
                        .with_static_gas(AFTER_EXECUTE_NEXT_WITHDRAW_GAS)
                        // `need` here is informational; we do not track it across the defer
                        .after_exec_withdraw_req(op_id, market_index, U128(0)),
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
            .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
            .ft_balance_of(env::current_account_id())
            .then(
                ext_self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
                    .after_skim_balance(token, self.skim_recipient.clone()),
            )
    }

    /// Allocates assets across markets according to the provided weights.
    /// If `amount` is provided, it is used as the target amount for each market.
    /// Otherwise, the vault will attempt to allocate as much as possible.
    ///
    /// NOTE: Each allocation takes roughly [common::vault::ALLOCATE_GAS] gas. (~21 TGAS)
    /// So in one allocation cycle, we should do at most ~12 market allocations.
    /// This is a conservative estimate, and may need to be tweaked.
    ///
    ///
    /// NOTE: When we rewrite this we should use a delta based approach
    #[payable]
    pub fn allocate(
        &mut self,
        weights: AllocationWeights,
        amount: Option<U128>,
    ) -> PromiseOrValue<()> {
        require_at_least(ALLOCATE_GAS);
        Self::assert_allocator();
        self.ensure_idle();

        let total = self.clamp_allocation_total(amount.map(|x| x.0));

        if weights.is_empty() {
            if total == 0 {
                return self.stop_and_exit(Some(&Error::ZeroAmount));
            }
            let op_id = self.next_op_id;
            Event::AllocationRequestedQueue {
                op_id: op_id.into(),
                total: U128(total),
            }
            .emit();
            self.plan = None;
            return self.start_allocation(total);
        }

        // Non-empty weights: validate and build plan.
        let weights = weights
            .into_iter()
            .map(|(m, w)| (m, u128::from(w)))
            .collect::<HashMap<_, _>>();

        let sum_weights: u128 = weights.values().sum();
        if sum_weights == 0 {
            env::panic_str("Sum of weights is zero");
        }
        if total == 0 {
            env::panic_str("No funds to allocate");
        }

        let op_id = self.next_op_id;
        let weights_for_event: Vec<(AccountId, U128)> =
            weights.iter().map(|(m, w)| (m.clone(), U128(*w))).collect();
        Event::AllocationPlanSet {
            op_id: op_id.into(),
            total: U128(total),
            plan: weights_for_event,
        }
        .emit();
        self.plan = Some(weights.into_iter().collect());

        self.start_allocation(total)
    }
    // Advance next_withdraw_to_execute to the next present id and return it, or None if none
    fn peek_next_pending_withdrawal_id(&mut self) -> Option<u64> {
        let mut id = self.next_withdraw_to_execute;
        while id < self.next_withdraw_id {
            if self.pending_withdrawals.get(&id).is_some() {
                self.next_withdraw_to_execute = id; // head points at a live entry
                return Some(id);
            }
            id = id.saturating_add(1);
        }
        self.next_withdraw_to_execute = id; // no entries
        None
    }

    // Remove the in-flight pending (success or explicit abort) and advance head past it
    fn remove_inflight_and_advance_head(&mut self) {
        if let Some(id) = self.current_withdraw_inflight.take() {
            let _ = self.pending_withdrawals.remove(&id);
            self.next_withdraw_to_execute = id.saturating_add(1);
            env::log_str(&format!("WithdrawalDequeued id={id}"));
        }
    }

    // Keep the head pending but clear in-flight so it can be retried later
    fn park_inflight_head_for_retry(&mut self) {
        if self.current_withdraw_inflight.is_some() {
            env::log_str(&format!(
                "WithdrawalParked id={}",
                self.current_withdraw_inflight.unwrap()
            ));
        }
        self.current_withdraw_inflight = None;
        // next_withdraw_to_execute remains pointing at the same id
    }
}

/* ----- Views ----- */
#[near]
impl Contract {
    #[allow(clippy::expect_used, reason = "No side effects")]
    pub fn get_configuration(&self) -> VaultConfiguration {
        let meta = self.get_metadata();
        VaultConfiguration {
            owner: self
                .own_get_owner()
                .unwrap_or_else(|| env::panic_str("Owner not set in get_configuration")),
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
            initial_timelock_ns: self.timelock_ns.into(),
            fee_recipient: self.fee_recipient.clone(),
            skim_recipient: self.skim_recipient.clone(),
            name: meta.name,
            symbol: meta.symbol,
            decimals: NonZeroU8::new(meta.decimals).unwrap(),
            mode: self.mode.clone(),
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

    /// Returns the maximum additional amount that can be deposited across all markets given current caps.
    pub fn get_max_deposit(&self) -> U128 {
        let total = self
            .supply_queue
            .iter()
            .fold(0u128, |acc, m| match self.markets.get(m) {
                Some(cfg) if cfg.cap.0 > 0 => {
                    let cur = *self.market_supply.get(m).unwrap_or(&0);
                    acc + cfg.cap.0.saturating_sub(cur)
                }
                _ => acc,
            });
        U128(total)
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
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EscrowSettlement {
    pub to_burn: u128,
    pub refund: u128,
}

impl From<EscrowSettlement> for (u128, u128) {
    fn from(tuple: EscrowSettlement) -> Self {
        (tuple.to_burn, tuple.refund)
    }
}

/* ----- Private Helpers ----- */
impl Contract {
    fn cfg_mut(&mut self, id: &AccountId) -> &mut MarketConfiguration {
        self.markets
            .get_mut(id)
            .unwrap_or_else(|| env::panic_str(&format!("Config not found for market {id}")))
    }

    fn cfg(&self, id: &AccountId) -> &MarketConfiguration {
        self.markets
            .get(id)
            .unwrap_or_else(|| env::panic_str(&format!("Config not found for market {id}")))
    }

    // Principal (vault-supplied) units currently recorded for a market
    fn principal_of(&self, market: &AccountId) -> u128 {
        *self.market_supply.get(market).unwrap_or(&0)
    }

    fn cap_of(&self, market: &AccountId) -> u128 {
        self.markets.get(market).map_or(0, |c| c.cap.0)
    }

    // Remaining room until cap for a market
    fn room_of(&self, market: &AccountId) -> u128 {
        self.cap_of(market)
            .saturating_sub(self.principal_of(market))
    }

    fn in_withdraw_queue(&self, market: &AccountId) -> bool {
        self.withdraw_queue.iter().any(|m| m == market)
    }

    // Add market to withdraw_queue and adjust last_total_assets if re-adding with existing principal
    pub(crate) fn add_market_to_withdraw_queue(
        &mut self,
        market: &AccountId,
        before_principal: u128,
    ) {
        if self.in_withdraw_queue(market) {
            Event::MarketAlreadyInWithdrawQueue {
                market: market.clone(),
            }
            .emit();
            return;
        }
        self.withdraw_queue.push(market.clone());
        Event::WithdrawQueueMarketAdded {
            market: market.clone(),
        }
        .emit();
        self.aum
            .clone()
            .paper_aum_undercounting(self, &before_principal);
    }

    /// Enqueue a vault-level pending withdrawal request (escrow already taken).
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
    fn compute_burn_shares(
        &self,
        escrow_shares: u128,
        collected: u128,
        requested_total: u128,
    ) -> u128 {
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
            self.mint(&Nep141Mint::new(minted, &recipient));
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
            env::panic_str(&format!(
                "Invariant: Only one op in flight; current op_state = {:?}",
                self.op_state
            ));
        }
    }

    fn start_allocation(&mut self, amount: u128) -> PromiseOrValue<()> {
        if amount == 0 {
            return self.stop_and_exit(Some(&Error::ZeroAmount));
        }
        self.ensure_idle();

        require!(
            amount <= self.idle_balance,
            "Policy violation: reserve amount must be <= idle_balance"
        );
        // Deduct from idle_balance upfront
        self.idle_balance -= amount;

        let op_id = self.next_op_id;
        self.next_op_id += 1;
        self.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: 0,
            remaining: amount,
        });
        Event::AllocationStarted {
            op_id: op_id.into(),
            remaining: U128(amount),
        }
        .emit();
        self.step_allocation()
    }

    /// build a supply `transfer_call` and chain `after_supply_1_check`
    fn supply_and_then(&self, market: &AccountId, amount: u128, op_id: u64, index: u32) -> Promise {
        self::require_at_least(AFTER_SUPPLY_1_CHECK_GAS.saturating_add(GAS_FOR_FT_TRANSFER_CALL));
        self.underlying_asset
            .transfer_call(
                market,
                U128(amount).into(),
                Some(
                    #[allow(clippy::expect_used, reason = "Infallible")]
                    serde_json::to_string(&templar_common::market::DepositMsg::Supply)
                        .unwrap_or_else(|e| env::panic_str(&e.to_string()))
                        .as_str(),
                ),
            )
            .then(
                ext_self::ext(env::current_account_id())
                    .with_static_gas(AFTER_SUPPLY_1_CHECK_GAS)
                    // .with_unused_gas_weight(0)
                    .after_supply_1_check(op_id, index, U128(amount)),
            )
    }

    // Step allocation when a weighted plan is present.
    fn step_allocation_with_plan(
        &mut self,
        op_id: u64,
        index: u32,
        remaining: u128,
    ) -> PromiseOrValue<()> {
        if let Some(plan) = &self.plan {
            let idx = index as usize;
            if let Some((market, weight)) = plan.get(idx) {
                let market_id = market.clone();

                // Sum weights of remaining markets in the plan (including current)
                let mut sum_w: u128 = 0;
                for (_, w) in plan.iter().skip(idx) {
                    sum_w = sum_w.saturating_add(*w);
                }

                // Compute weighted target for this step. For the last market (or zero sum), take all remaining.
                let target = if sum_w == 0 || idx + 1 == plan.len() {
                    remaining
                } else {
                    mul_div_floor(remaining.into(), (*weight).into(), sum_w.into()).into()
                };

                let room = self.room_of(&market_id);
                let to_supply = room.min(target);

                Event::AllocationStepPlanned {
                    op_id: op_id.into(),
                    index,
                    market: market_id.clone(),
                    target: U128(target),
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
                    });
                    return self.step_allocation();
                }

                PromiseOrValue::Promise(self.supply_and_then(&market_id, to_supply, op_id, index))
            } else {
                // Plan exhausted; stop and reconcile remaining in stop_and_exit
                self.stop_and_exit::<Error>(None)
            }
        } else {
            self.stop_and_exit(Some(&Error::NotAllocating))
        }
    }

    // Step allocation using the supply_queue order.
    fn step_allocation_from_queue(
        &mut self,
        op_id: u64,
        index: u32,
        remaining: u128,
    ) -> PromiseOrValue<()> {
        if let Some(market) = self.supply_queue.get(index) {
            let room = self.room_of(market);
            let to_supply = room.min(remaining);

            // Emit planned step event (queue-based)
            Event::AllocationStepPlanned {
                op_id: op_id.into(),
                index,
                market: market.clone(),
                target: U128(remaining),
                room: U128(room),
                to_supply: U128(to_supply),
                remaining_before: U128(remaining),
                planned: false,
            }
            .emit();

            if to_supply == 0 {
                Event::AllocationStepSkipped {
                    op_id: op_id.into(),
                    index,
                    market: market.clone(),
                    reason: "no-room".to_string(),
                    remaining: U128(remaining),
                }
                .emit();

                self.op_state = OpState::Allocating(AllocatingState {
                    op_id,
                    index: index + 1,
                    remaining,
                });
                return self.step_allocation();
            }

            PromiseOrValue::Promise(self.supply_and_then(market, to_supply, op_id, index))
        } else {
            self.stop_and_exit::<Error>(None)
        }
    }

    fn step_allocation(&mut self) -> PromiseOrValue<()> {
        let (op_id, index, remaining) = match &self.op_state {
            OpState::Allocating(AllocatingState {
                op_id,
                index,
                remaining,
            }) => (*op_id, *index, *remaining),
            _ => return self.stop_and_exit(Some(&Error::NotAllocating)),
        };

        if remaining == 0 {
            // All funds allocated successfully
            return self.stop_and_exit::<Error>(None);
        }

        if self.plan.is_some() {
            self.step_allocation_with_plan(op_id, index, remaining)
        } else {
            self.step_allocation_from_queue(op_id, index, remaining)
        }
    }

    fn start_withdraw(
        &mut self,
        amount: u128,
        receiver: &AccountId,
        owner: &AccountId,
        escrow_shares: u128,
    ) -> PromiseOrValue<()> {
        if amount == 0 {
            return self.stop_and_exit(Some(&Error::ZeroAmount));
        }
        self.ensure_idle();
        let op_id = self.next_op_id;
        self.next_op_id += 1;

        // Invariant: Idle-first reservation does not mutate idle_balance until payout succeeds.
        let used_idle = self.idle_balance.min(amount);
        let remaining = amount.saturating_sub(used_idle);
        let collected = used_idle;

        self.pending_market_exec.clear();

        self.op_state = OpState::Withdrawing(WithdrawingState {
            op_id,
            index: Default::default(),
            remaining,
            receiver: receiver.clone(),
            collected,
            owner: owner.clone(),
            escrow_shares,
        });
        self.step_withdraw()
    }

    fn step_withdraw(&mut self) -> PromiseOrValue<()> {
        let (op_id, index, remaining, receiver, collected, owner, escrow_shares) =
            match &self.op_state {
                OpState::Withdrawing(WithdrawingState {
                    op_id,
                    index,
                    remaining,
                    receiver,
                    collected,
                    owner,
                    escrow_shares,
                }) => (
                    *op_id,
                    *index,
                    *remaining,
                    receiver.clone(),
                    *collected,
                    owner.clone(),
                    *escrow_shares,
                ),
                _ => return self.stop_and_exit(Some(&Error::NotWithdrawing)),
            };

        if remaining == 0 {
            self.op_state = OpState::Payout(PayoutState {
                op_id,
                receiver: receiver.clone(),
                amount: collected,
                owner: owner.clone(),
                escrow_shares,
                burn_shares: escrow_shares,
            });
            return PromiseOrValue::Promise(
                self.underlying_asset
                    .transfer(receiver.clone(), U128(collected).into())
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(AFTER_SEND_TO_USER_GAS)
                            .after_send_to_user(op_id, receiver, U128(collected)),
                    ),
            );
        }
        if let Some(market) = self.withdraw_queue.get(index) {
            let have = self.market_supply.get(market).unwrap_or(&0);
            let to_request = have.min(&remaining);
            if to_request == &0 {
                self.op_state = OpState::Withdrawing(WithdrawingState {
                    op_id,
                    index: index + 1,
                    remaining,
                    receiver,
                    collected,
                    owner,
                    escrow_shares,
                });
                env::log_str(&format!(
                    "Skipping withdrawal for market {market} (have {have}, remaining {remaining})"
                ));
                return self.step_withdraw();
            }
            PromiseOrValue::Promise(
                templar_common::market::ext_market::ext(market.clone())
                    .with_static_gas(CREATE_WITHDRAW_REQ_GAS)
                    .create_supply_withdrawal_request(BorrowAssetAmount::from(U128(*to_request)))
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(AFTER_CREATE_WITHDRAW_REQ_GAS)
                            .after_create_withdraw_req(op_id, index, U128(*to_request)),
                    ),
            )
        } else {
            let requested = collected.saturating_add(remaining);
            let burn_shares = self.compute_burn_shares(escrow_shares, collected, requested);

            self.pay_collected(
                op_id,
                &receiver,
                collected,
                &owner,
                escrow_shares,
                burn_shares,
                |_self| {
                    // Park the head pending: keep escrowed shares, stay in queue, try again later
                    _self.op_state = OpState::Idle;
                    _self.park_inflight_head_for_retry();
                    PromiseOrValue::Value(())
                },
            )
        }
    }

    ///  If we collected something, pay it out now and burn proportional shares or do something else
    fn pay_collected(
        &mut self,
        op_id: u64,
        receiver: &AccountId,
        collected: u128,
        owner: &AccountId,
        escrow_shares: u128,
        burn_shares: u128,
        or_else: impl FnOnce(&mut Self) -> PromiseOrValue<()>,
    ) -> PromiseOrValue<()> {
        if collected > 0 {
            self.op_state = OpState::Payout(PayoutState {
                op_id,
                receiver: receiver.clone(),
                amount: collected,
                owner: owner.clone(),
                escrow_shares,
                burn_shares,
            });
            PromiseOrValue::Promise(
                self.underlying_asset
                    .transfer(receiver.clone(), U128(collected).into())
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(AFTER_SEND_TO_USER_GAS)
                            .after_send_to_user(op_id, receiver.clone(), U128(collected)),
                    ),
            )
        } else {
            or_else(self)
        }
    }
}

impl near_sdk_contract_tools::hook::Hook<Self, Nep145ForceUnregister<'_>> for Contract {
    fn hook<R>(_: &mut Self, _: &Nep145ForceUnregister, _: impl FnOnce(&mut Self) -> R) -> R {
        // Invariant: Force unregister must fail to preserve FT ledger integrity.
        env::panic_str("force unregistration is not supported")
    }
}

#[cfg(test)]
mod tests;
