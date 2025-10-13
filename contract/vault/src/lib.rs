#![allow(clippy::needless_pass_by_value)]

use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env,
    json_types::U128,
    near, serde_json,
    store::{IterableMap, LookupMap, Vector},
    AccountId, BorshStorageKey, Gas, IntoStorageKey, NearToken, PanicOnDefault, Promise,
    PromiseOrValue,
};
use near_sdk_contract_tools::{
    ft::{
        nep141::GAS_FOR_FT_TRANSFER_CALL, ContractMetadata, FungibleToken, Nep141Controller,
        Nep148Controller,
    },
    standard::nep145::{Nep145Controller, Nep145ForceUnregister},
    Owner, Rbac,
};
use near_sdk_contract_tools::{owner::Owner, rbac};
use near_sdk_contract_tools::{owner::OwnerExternal, rbac::Rbac};
use templar_common::{
    asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset},
    vault::{
        ext_self, AllocationMode, AllocationPlan, AllocationWeights, Error, Event,
        MarketConfiguration, OpState, PendingValue, TimestampNs, VaultConfiguration, MAX_QUEUE_LEN,
        MAX_TIMELOCK_NS, MIN_TIMELOCK_NS,
    },
};
pub use wad::*;

pub mod aux;
pub mod impl_callbacks;
pub mod impl_token_receiver;
pub mod wad;

#[cfg(test)]
mod test_utils;

#[derive(Debug, Clone)]
#[near(serializers = [json, borsh])]
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

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct PendingWithdrawal {
    pub owner: AccountId,
    pub receiver: AccountId,
    pub escrow_shares: u128,
    pub expected_assets: u128,
    pub requested_at: u64,
}

#[derive(PanicOnDefault, FungibleToken, Owner, Rbac)]
// FIXME: #[nep145(force_unregister_hook = "Self")]
#[rbac(roles = "Role", crate = "crate")]
#[near(contract_state)]
/// Vault contract that issues shares over an underlying fungible asset and allocates liquidity
/// across configured markets. Implements 4626-like deposit/withdraw semantics.
pub struct Contract {
    mode: AllocationMode,
    plan: Option<AllocationPlan>,

    underlying_asset: FungibleAsset<BorrowAsset>,
    /// configuration per market (market ID -> MarketConfig)
    config: IterableMap<AccountId, MarketConfiguration>,

    // TODO: decimal offset for virtual shares
    /// Performance fee (as WAD fraction)
    performance_fee: wad::WADFraction,
    fee_recipient: AccountId,
    skim_recipient: AccountId,
    /// Last recorded total assets (for fee accrual)
    last_total_assets: u128,

    // Virtual offsets used only in conversions/previews to harden edge cases
    virtual_shares: u128,
    virtual_assets: u128,

    /// Any pending change to the vault's cap, TODO: u256
    pending_cap: IterableMap<AccountId, PendingValue<u128>>,
    /// Any pending change to the vault's timelock
    pending_timelock: Option<PendingValue<TimestampNs>>,
    /// Any pending change to the vault's guardian
    pending_guardian: Option<PendingValue<AccountId>>,
    /// Current timelock duration for governance actions (ns)
    timelock_ns: TimestampNs,

    // Ordered list of market IDs for deposit allocation
    supply_queue: Vector<AccountId>,
    // Ordered list of market IDs for withdrawal prioritytr
    withdraw_queue: Vector<AccountId>,

    // vault's supplied principal per market (borrow-asset units)
    market_supply: LookupMap<AccountId, u128>,

    // underlying held by vault
    idle_balance: u128,
    op_state: OpState,
    next_op_id: u64,

    // Storage usage
    storage_usage_supply: u64,
    storage_usage_role: u64,

    // Pending withdrawals queue (vault-level, FIFO by id)
    pending_withdrawals: IterableMap<u64, PendingWithdrawal>,
    next_withdraw_id: u64,
    next_withdraw_to_execute: u64,
}

#[near]
impl Contract {
    #[allow(clippy::unwrap_used, reason = "Infallible")]
    #[allow(clippy::too_many_arguments, reason = "Constructor")]
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
    pub fn new(configuration: VaultConfiguration) -> Self {
        let VaultConfiguration {
            owner,
            curator,
            guardian,
            underlying_token,
            initial_timelock_sec,
            fee_recipient,
            skim_recipient,
            name,
            symbol,
            decimals,
            mode,
        } = configuration;

        let timelock_ns = u64::from(initial_timelock_sec) * 1_000_000_000;
        assert!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&timelock_ns),
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

        // TODO: this but with roles and other storage we set
        // let storage_usage_1 = env::storage_usage();
        // market.finalized_snapshots.flush();
        // let storage_usage_2 = env::storage_usage();
        // let storage_usage_snapshot = storage_usage_2.saturating_sub(storage_usage_1);
        let storage_usage_supply = env::storage_usage();
        let storage_usage_role = env::storage_usage();

        let mut contract = Self {
            underlying_asset: underlying_token,
            timelock_ns,
            performance_fee: Default::default(),
            fee_recipient,
            skim_recipient,
            config: IterableMap::new(key!(Config)),
            pending_cap: IterableMap::new(key!(PendingCaps)),
            pending_timelock: None,
            pending_guardian: None,
            supply_queue: Vector::new(key!(SupplyQueue)),
            withdraw_queue: Vector::new(key!(WithdrawQueue)),
            market_supply: LookupMap::new(key!(MarketSupply)),
            last_total_assets: 0,
            virtual_shares: 1,
            virtual_assets: 1,
            idle_balance: 0,
            op_state: OpState::Idle,
            next_op_id: 1,
            storage_usage_supply,
            storage_usage_role,
            mode,
            plan: None,

            // Pending withdrawals init
            pending_withdrawals: IterableMap::new(key!(PendingWithdrawals)),
            next_withdraw_id: 0,
            next_withdraw_to_execute: 0,
        };
        contract.set_metadata(&ContractMetadata::new(name, symbol, decimals));
        Owner::init(&mut contract, &owner);
        Rbac::add_role(&mut contract, &curator, &Role::Curator);
        Rbac::add_role(&mut contract, &curator, &Role::Allocator);
        Rbac::add_role(&mut contract, &guardian, &Role::Guardian);

        contract
    }

    /// Sets the Curator account. Also grants/removes the Allocator role accordingly.
    pub fn set_curator(&mut self, account: AccountId) {
        Self::require_owner();
        Self::with_members_of(&Role::Curator, |members| {
            assert!(
                members.len() < 2,
                "Invariant violation: Cannot Have more than 1 Curator"
            );
            assert!(
                !members.contains(&account),
                "Curator already set to this account"
            );
            members.iter().for_each(|m| {
                self.remove_role(&m, &Role::Curator);
                self.remove_role(&m, &Role::Allocator);
            });
        });
        Self::add_role(self, &account, &Role::Curator);
        Self::add_role(self, &account, &Role::Allocator);
        Event::CuratorSet {
            account: account.clone(),
        }
        .emit();
        Event::AllocatorRoleSet {
            account,
            allowed: true,
        }
        .emit();
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
            assert!(
                members.len() < 2,
                "Invariant violation: Cannot Have more than 1 Guardian"
            );
            assert!(!members.contains(&new_g), "Already set to this address");
            guardian_occupied = !members.is_empty();
        });
        assert!(
            self.pending_guardian.is_none(),
            "Guardian change already pending"
        );
        if guardian_occupied {
            let valid_at = env::block_timestamp() + self.timelock_ns;
            self.pending_guardian = Some(PendingValue {
                value: new_g,
                valid_at,
            });
        } else {
            Self::add_role(self, &new_g, &Role::Guardian);
        }
    }

    /// Accepts the pending Guardian change after the timelock has elapsed.
    pub fn accept_guardian(&mut self) {
        Self::require_owner();

        let p = self.pending_guardian.clone();

        if let Some(p) = &p {
            assert!(env::block_timestamp() >= p.valid_at, "not yet");
            Self::with_members_of(&Role::Guardian, |members| {
                members.iter().for_each(|m| {
                    self.remove_role(&m, &Role::Guardian);
                });
                Self::add_role(self, &p.value, &Role::Guardian);
            });
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
        assert!(
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
    pub fn set_fee_recipient(&mut self, account: AccountId) {
        Self::require_owner();
        assert!(account != self.fee_recipient, "Already set to this address");

        if self.performance_fee != 0 {
            // Accrue any pending fees to current recipient before changing (so current recipient gets up to now)
            self.internal_accrue_fee();
        }
        Event::FeeRecipientSet {
            account: account.clone(),
        }
        .emit();
        self.fee_recipient = account;
    }

    /// Sets the performance fee as a WAD fraction (1e18 = 100%). Accrues fees at the old rate first.
    pub fn set_performance_fee(&mut self, fee: U128) {
        Self::require_owner();

        let fee: u128 = fee.into();

        assert!(fee != self.performance_fee, "Fee already set to this value");
        // FIXME: dynamic based on underlying
        assert!(fee <= (wad::WAD / 10), "fee too high");

        // Accrue any pending fees with old rate before changing
        self.internal_accrue_fee();
        self.performance_fee = fee;
        Event::PerformanceFeeSet { fee: U128(fee) }.emit();
    }

    /* ----- Timelocks / Pending ----- */
    /// Proposes a new governance timelock in seconds.
    /// If increasing, applies immediately; if decreasing, starts a timelock equal to the current duration.
    pub fn submit_timelock(&mut self, new_timelock_secs: u32) {
        Self::require_owner();
        let as_nanos = u64::from(new_timelock_secs) * 1_000_000_000;

        assert!(as_nanos != self.timelock_ns, "Already set to this value");
        assert!(
            self.pending_timelock.is_none(),
            "Timelock change already pending"
        );
        assert!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&as_nanos),
            "Timelock out of bounds"
        );
        if as_nanos > self.timelock_ns {
            self.timelock_ns = as_nanos;
            Event::TimelockSet {
                seconds: new_timelock_secs,
            }
            .emit();
        } else {
            let valid_at = env::block_timestamp() + self.timelock_ns;
            self.pending_timelock = Some(PendingValue {
                value: as_nanos,
                valid_at,
            });
            Event::TimelockChangeSubmitted {
                new_seconds: new_timelock_secs,
                valid_at,
            }
            .emit();
        }
    }

    /// Accepts a pending timelock change after it becomes valid.
    pub fn accept_timelock(&mut self) {
        Self::require_owner();
        if let Some(p) = &self.pending_timelock {
            assert!(
                env::block_timestamp() >= p.valid_at,
                "Timelock not elapsed yet"
            );
            self.timelock_ns = p.value;
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
    pub fn submit_cap(&mut self, market: AccountId, new_cap: U128) {
        Self::assert_curator_or_owner();
        self.ensure_idle();
        let config = match self.config.get_mut(&market) {
            None => {
                self.config
                    .insert(market.clone(), MarketConfiguration::default());
                Event::MarketCreated {
                    market: market.clone(),
                }
                .emit();
                #[allow(clippy::unwrap_used, reason = "No side effects")]
                self.config.get_mut(&market).unwrap()
            }
            Some(config) => config,
        };

        assert!(
            self.pending_cap.get(&market).is_none(),
            "Invariant violation: A cap change is already pending for this market"
        );
        assert!(
            config.removable_at == 0,
            "Market removal pending, cannot change cap"
        );
        let new_cap = new_cap.0;
        assert!(new_cap != config.cap, "New cap is same as current");

        if new_cap < config.cap {
            // If lowering the cap, we can apply the delta immediately

            config.cap = new_cap;
            // Disable market if cap is zero
            if new_cap == 0 {
                config.enabled = false;
            }
        } else {
            let valid_at = env::block_timestamp() + self.timelock_ns;
            self.pending_cap.insert(
                market.clone(),
                PendingValue {
                    value: new_cap,
                    valid_at,
                },
            );
            Event::SupplyCapRaiseSubmitted {
                market: market.clone(),
                new_cap: U128(new_cap),
                valid_at,
            }
            .emit();
        }
    }

    /// Accepts a pending cap increase for `market` once the timelock has elapsed.
    pub fn accept_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        self.ensure_idle();
        if let Some(pending) = self.pending_cap.get(&market) {
            assert!(
                env::block_timestamp() >= pending.valid_at,
                "Timelock not elapsed for cap change"
            );

            #[allow(clippy::expect_used, reason = "No side effects")]
            let cfg = self.config.get_mut(&market).expect("Market not found");

            cfg.cap = pending.value;
            if pending.value > 0 {
                // If enabling or raising cap above 0, mark enabled and add to withdraw_queue if not already present
                if !cfg.enabled {
                    cfg.enabled = true;
                    let mut added = false;
                    if self.withdraw_queue.iter().any(|m| m == &market) {
                        Event::MarketEnabled {
                            market: market.clone(),
                        }
                        .emit();
                        Event::MarketAlreadyInWithdrawQueue {
                            market: market.clone(),
                        }
                        .emit();
                    } else {
                        self.withdraw_queue.push(market.clone());
                        Event::MarketEnabled {
                            market: market.clone(),
                        }
                        .emit();
                        Event::WithdrawQueueMarketAdded {
                            market: market.clone(),
                        }
                        .emit();
                        added = true;
                    }

                    // Only adjust last_total_assets if we just re-added the market to the withdraw queue
                    if added {
                        let current = self.market_supply.get(&market).unwrap_or(&0);
                        self.last_total_assets = self.last_total_assets.saturating_add(*current);
                    }
                }
                cfg.removable_at = 0;
            } else {
                cfg.enabled = false;
            }
            Event::SupplyCapSet {
                market: market.clone(),
                new_cap: U128(pending.value),
            }
            .emit();
            self.pending_cap.remove(&market);
        } else {
            env::panic_str("No pending cap change for this market");
        }
    }

    /// Revokes any pending cap change for `market`.
    pub fn revoke_pending_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        if self.pending_cap.get(&market).is_some() {
            self.pending_cap.remove(&market);
        }
    }

    // To remove a market entirely, the curator:
    //- first sets its cap to 0 (disabling new deposits)
    //- then calls submit_market_removal.
    // > This starts a timelock (using the vault’s timelock)
    // - after which the market can be removed from the withdraw_queue (assuming any funds have been withdrawn)
    /// Begins the process to remove `market` from the withdraw queue.
    /// Requires cap == 0 and no pending cap changes; starts a timelock.
    pub fn submit_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        let cfg = self
            .config
            .get_mut(&market)
            .unwrap_or_else(|| env::panic_str("unknown market"));
        assert!(
            cfg.removable_at == 0,
            "Removal already pending for this market"
        );
        assert!(
            cfg.cap == 0,
            "Cannot remove market with non-zero cap (disable deposits first)"
        );
        assert!(cfg.enabled, "Market not enabled or already removed");
        assert!(
            self.pending_cap.get(&market).is_none(),
            "Cap change pending for this market"
        );
        cfg.removable_at = env::block_timestamp() + self.timelock_ns;
        Event::MarketRemovalSubmitted {
            market: market.clone(),
            removable_at: cfg.removable_at,
        }
        .emit();
    }
    /// Revokes a pending market removal for `market`.
    pub fn revoke_pending_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        if let Some(cfg) = self.config.get_mut(&market) {
            cfg.removable_at = 0;
        }
        Event::MarketRemovalRevoked { market }.emit();
    }

    /// Sets the ordered supply (allocation) queue.
    /// Rejects duplicates and markets without a positive cap. Requires the vault to be idle.
    pub fn set_supply_queue(&mut self, markets: Vec<AccountId>) {
        Self::assert_allocator();
        self.ensure_idle();
        assert!(markets.len() <= MAX_QUEUE_LEN, "too long");

        // Invariant: supply_queue has no duplicates; allocation order remains meaningful
        let mut seen = std::collections::HashSet::new();
        for m in &markets {
            if !seen.insert(m.clone()) {
                env::panic_str(&format!("Duplicate market {m}"));
            }
        }

        self.supply_queue.clear();
        for m in &markets {
            let cap = self.config.get(m).map_or(0, |c| c.cap);
            assert!(cap > 0, "unauthorized market");
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
    pub fn set_withdraw_queue(&mut self, queue: Vec<AccountId>) {
        Self::assert_allocator();
        self.ensure_idle();
        assert!(
            queue.len() <= MAX_QUEUE_LEN,
            "Withdraw queue length exceeds max"
        );

        let mut seen = std::collections::HashSet::new();
        for id in &queue {
            if !seen.insert(id.clone()) {
                env::panic_str(&format!("Duplicate market {id}"));
            }
        }

        // Snapshot current withdraw queue into a set for membership checks
        let current: std::collections::HashSet<AccountId> =
            self.withdraw_queue.iter().cloned().collect();

        // Each id in the new queue must correspond to a known market
        for id in &queue {
            assert!(self.config.get(id).is_some(), "Unknown market in new queue");
        }

        // Enforce invariant: withdraw_queue must include all enabled or holding markets
        for (id, cfg) in self.config.iter() {
            let has_supply = *self.market_supply.get(id).unwrap_or(&0) > 0;
            if cfg.enabled || has_supply {
                assert!(
                    seen.contains(id),
                    "Withdraw queue must include all enabled or holding markets"
                );
            }
        }

        // For every market being removed, enforce safety invariants before removal
        for id in current.difference(&seen).cloned().collect::<Vec<_>>() {
            #[allow(clippy::expect_used, reason = "No side effects")]
            let config = self.config.get_mut(&id).expect("Market not found");

            assert!(config.cap == 0, "Cannot remove market with non-zero cap");
            assert!(
                self.pending_cap.get(&id).is_none(),
                "Cannot remove market with pending cap change"
            );
            let position = *self.market_supply.get(&id).unwrap_or(&0);
            if position > 0 {
                assert!(
                    config.removable_at > 0,
                    "Market still has supply but no removal scheduled"
                );
                assert!(
                    env::block_timestamp() >= config.removable_at,
                    "Removal timelock not elapsed for market"
                );
            }
            // Remove market configuration
            self.config.remove(&id);
        }

        // Replace withdraw_queue atomically
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
    pub fn withdraw(&mut self, amount: U128, receiver: AccountId) -> PromiseOrValue<()> {
        let shares_needed = self.preview_withdraw(amount).0;
        self.redeem(U128(shares_needed), receiver)
    }

    /// Redeems `shares` for underlying assets sent to `receiver`.
    /// Shares are escrowed to the contract and only burned after successful payout.
    pub fn redeem(&mut self, shares: U128, receiver: AccountId) -> PromiseOrValue<()> {
        let shares = shares.0;

        let assets = self.convert_to_assets(U128(shares)).0;

        let owner = env::predecessor_account_id();

        // Move shares into vault escrow; do not burn yet
        #[allow(clippy::expect_used, reason = "No side effects")]
        self.transfer_unchecked(&owner, &env::current_account_id(), shares)
            .expect("Redeem failed to move shares into escrow");

        self.internal_accrue_fee();

        Event::RedeemRequested {
            shares: U128(shares),
            estimated_assets: U128(assets),
        }
        .emit();

        self.enqueue_pending_withdrawal(&owner, &receiver, shares, assets);
        PromiseOrValue::Value(())
    }

    /// Executes the next pending withdrawal request, if any, using the existing withdraw pipeline.
    /// This defers creating market-side withdrawal requests until explicitly invoked.
    pub fn execute_next_withdrawal_request(&mut self) -> PromiseOrValue<()> {
        self.ensure_idle();
        Self::assert_allocator();

        // Find the next present pending withdrawal by id
        let mut id = self.next_withdraw_to_execute;
        while id < self.next_withdraw_id {
            if let Some(pending) = self.pending_withdrawals.remove(&id) {
                // Advance the head pointer and start processing
                self.next_withdraw_to_execute = id.saturating_add(1);
                return self.start_withdraw(
                    pending.expected_assets,
                    pending.receiver,
                    pending.owner,
                    pending.escrow_shares,
                );
            }
            id = id.saturating_add(1);
            self.next_withdraw_to_execute = id;
        }

        PromiseOrValue::Value(())
    }

    /// Sends the entire balance of `token` held by the vault to the `skim_recipient`.
    pub fn skim(&mut self, token: AccountId) -> Promise {
        Self::require_owner();
        ext_ft_core::ext(token.clone())
            .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
            .ft_balance_of(env::current_account_id())
            .then(
                ext_self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
                    .after_skim_balance(token, self.skim_recipient.clone()),
            )
    }

    pub fn allocate(
        &mut self,
        weights: AllocationWeights,
        amount: Option<U128>,
    ) -> PromiseOrValue<()> {
        Self::assert_allocator();
        self.ensure_idle();

        // If no weights provided, use queue order; clamp total and emit request event.
        if weights.is_empty() {
            let requested: u128 = amount.map_or(self.idle_balance, |x| x.0);
            let max_room = self.get_max_deposit().0;
            let total = requested.min(self.idle_balance).min(max_room);
            if total == 0 {
                return self.stop_and_exit(Some(&Error::ZeroAmount));
            }
            let op_id = self.next_op_id;
            Event::AllocationRequestedQueue {
                op_id,
                total: U128(total),
            }
            .emit();
            self.plan = None;
            self.start_allocation(total)
        } else {
            // Validate unique markets and accumulate weight sum
            let mut seen = std::collections::HashSet::new();
            let mut sum_w: u128 = 0;

            for (m, w) in &weights {
                if !seen.insert(m.clone()) {
                    env::panic_str(&format!("Duplicate market in weights: {m}"));
                }
                sum_w = sum_w.saturating_add(u128::from(*w));
            }
            if sum_w == 0 {
                env::panic_str("Sum of weights is zero");
            }

            // Clamp total allocation by idle balance and aggregate room
            let requested: u128 = amount.map_or(self.idle_balance, |x| x.0);
            let max_room = self.get_max_deposit().0;
            let total = requested.min(self.idle_balance).min(max_room);
            if total == 0 {
                env::panic_str("No funds to allocate");
            }

            // Emit request and plan events
            let op_id = self.next_op_id;
            let weights_for_event: Vec<(AccountId, U128)> = weights
                .iter()
                .map(|(m, w)| (m.clone(), U128((*w).into())))
                .collect();
            Event::AllocationRequestedWeighted {
                op_id,
                total: U128(total),
                weights: weights_for_event.clone(),
            }
            .emit();
            Event::AllocationPlanSet {
                op_id,
                plan: weights_for_event,
            }
            .emit();

            // Store an ephemeral plan of (market, weight) to drive weighted allocation.
            let plan: AllocationPlan = weights
                .into_iter()
                .map(|(m, w)| (m, u128::from(w)))
                .collect();

            self.plan = Some(plan);
            self.start_allocation(total)
        }
    }
}

/* ----- Views ----- */
#[near]
impl Contract {
    pub fn get_configuration(&self) -> VaultConfiguration {
        let timelock_sec = self.timelock_ns / 1_000_000_000;
        VaultConfiguration {
            owner: self.own_get_owner().expect("Owner not set"),
            curator: Self::with_members_of(&Role::Curator, |members| {
                assert!(
                    members.len() == 1,
                    "Invariant violation: Cannot Have more than 1 Curator"
                );
                members.iter().next().expect("Curator not set").clone()
            }),
            guardian: Self::with_members_of(&Role::Guardian, |members| {
                assert!(
                    members.len() == 1,
                    "Invariant violation: Cannot Have more than 1 Guardian"
                );
                members.iter().next().expect("Guardian not set").clone()
            }),
            underlying_token: self.underlying_asset.clone(),
            initial_timelock_sec: timelock_sec as u32,
            fee_recipient: self.fee_recipient.clone(),
            skim_recipient: self.skim_recipient.clone(),
            name: self.get_metadata().name,
            symbol: self.get_metadata().symbol,
            decimals: self.get_metadata().decimals,
            mode: self.mode.clone(),
        }
    }

    /// Returns total assets under management = idle balance + sum of market principals.
    pub fn get_total_assets(&self) -> U128 {
        // TODO: join
        let mut sum = self.idle_balance;
        self.withdraw_queue.iter().for_each(|m| {
            sum += self.market_supply.get(m).unwrap_or(&0);
        });
        U128(sum)
    }

    pub fn get_total_supply(&self) -> U128 {
        U128(self.total_supply())
    }

    /// Returns the maximum additional amount that can be deposited across all markets given current caps.
    pub fn get_max_deposit(&self) -> U128 {
        // TODO: join
        let mut total = 0u128;
        self.supply_queue.iter().for_each(|m| {
            if let Some(cfg) = self.config.get(m) {
                if cfg.cap > 0 {
                    let cur = self.market_supply.get(m).unwrap_or(&0);
                    if cfg.cap > *cur {
                        total += cfg.cap - cur;
                    }
                }
            }
        });
        U128(total)
    }

    /// Converts an amount of underlying assets to shares, flooring the result.
    /// Uses virtual offsets and fee-aware totals (pre-accrual simulation) like MetaMorpho.
    pub fn convert_to_shares(&self, assets: U128) -> U128 {
        let a: u128 = assets.0;
        if a == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(mul_div_floor(a, new_total_supply, new_total_assets))
    }

    /// Converts an amount of shares to underlying assets, flooring the result.
    /// Uses virtual offsets and fee-aware totals (pre-accrual simulation) like MetaMorpho.
    pub fn convert_to_assets(&self, shares: U128) -> U128 {
        let s: u128 = shares.0;
        if s == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(mul_div_floor(s, new_total_assets, new_total_supply))
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
        U128(crate::wad::mul_div_ceil(
            s,
            new_total_assets,
            new_total_supply,
        ))
    }

    /// Preview the number of shares required to withdraw `assets` (ceiled).
    /// Applies virtual offsets and fee-aware totals (pre-accrual simulation).
    pub fn preview_withdraw(&self, assets: U128) -> U128 {
        let a = assets.0;
        if a == 0 {
            return U128(0);
        }
        let (new_total_supply, new_total_assets) = self.effective_totals_fee_aware();
        U128(crate::wad::mul_div_ceil(
            a,
            new_total_supply,
            new_total_assets,
        ))
    }

    /// Preview the amount of assets received by redeeming `shares` (floored).
    /// Returns 0 if total supply is zero.
    pub fn preview_redeem(&self, shares: U128) -> U128 {
        self.convert_to_assets(shares)
    }
}

/* ----- Private Helpers ----- */
impl Contract {
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
            id,
            owner: owner.clone(),
            receiver: receiver.clone(),
            escrow_shares: U128(escrow_shares),
            expected_assets: U128(expected_assets),
            requested_at,
        }
        .emit();
    }

    /// Computes fee-aware effective totals for conversions, mimicking MetaMorpho:
    /// - Include fee shares that would be minted if fees accrued now.
    /// - Apply virtual offsets: +virtual_shares to supply and +virtual_assets to assets.
    fn effective_totals_fee_aware(&self) -> (u128, u128) {
        let cur = self.get_total_assets().0;
        let ts = self.total_supply();
        let fee_shares =
            crate::wad::compute_fee_shares(cur, self.last_total_assets, self.performance_fee, ts);
        let new_total_supply = ts
            .saturating_add(fee_shares)
            .saturating_add(self.virtual_shares);
        let new_total_assets = cur.saturating_add(self.virtual_assets);
        (new_total_supply, new_total_assets)
    }

    /* ----- Internal: fee, shares ----- */
    pub fn mint_shares(&mut self, to: &AccountId, amount: u128) {
        if amount == 0 {
            return;
        }
        #[allow(clippy::expect_used, reason = "No side effects")]
        self.deposit_unchecked(to, amount)
            .expect("Failed to mint shares");
    }

    pub fn internal_accrue_fee(&mut self) {
        // Invariant: Fees are minted only when total_assets() > last_total_assets (no fees on losses/flat).
        let cur = self.get_total_assets().0;
        let fee_shares = crate::wad::compute_fee_shares(
            cur,
            self.last_total_assets,
            self.performance_fee,
            self.total_supply(),
        );
        if fee_shares > 0 {
            self.mint_shares(&self.fee_recipient.clone(), fee_shares);
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
            env::panic_str("busy");
        }
    }

    fn start_allocation(&mut self, amount: u128) -> PromiseOrValue<()> {
        if amount == 0 {
            return self.stop_and_exit(Some(&Error::ZeroAmount));
        }
        self.ensure_idle();

        assert!(
            amount <= self.idle_balance,
            "Invariant Violation: reserve amount must be <= idle_balance"
        );
        self.idle_balance -= amount;

        let op_id = self.next_op_id;
        self.next_op_id += 1;
        self.op_state = OpState::Allocating {
            op_id,
            index: 0,
            remaining: amount,
        };
        Event::AllocationStarted {
            op_id,
            remaining: U128(amount),
        }
        .emit();
        self.step_allocation()
    }

    fn step_allocation(&mut self) -> PromiseOrValue<()> {
        let (op_id, index, remaining) = match &self.op_state {
            OpState::Allocating {
                op_id,
                index,
                remaining,
            } => (*op_id, *index, *remaining),
            _ => return self.stop_and_exit(Some(&Error::NotAllocating(self.op_state.clone()))),
        };

        if remaining == 0 {
            return self.stop_and_exit::<Error>(None);
        }

        // If a per-op allocation plan exists, use it as weighted priority; otherwise, fall back to supply_queue order.
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
                    mul_div_floor(remaining, *weight, sum_w)
                };

                let cap = self.config.get(&market_id).map_or(0, |c| c.cap);
                let cur = *self.market_supply.get(&market_id).unwrap_or(&0);
                let room = cap.saturating_sub(cur);
                let to_supply = room.min(target);

                // Emit planned step event
                Event::AllocationStepPlanned {
                    op_id,
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
                        op_id,
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

                    self.op_state = OpState::Allocating {
                        op_id,
                        index: index + 1,
                        remaining,
                    };
                    return self.step_allocation();
                }

                return PromiseOrValue::Promise(
                    self.underlying_asset
                        .transfer_call(
                            &market_id,
                            U128(to_supply).into(),
                            Some(
                                #[allow(clippy::expect_used, reason = "Infallible")]
                                serde_json::to_string(&templar_common::market::DepositMsg::Supply)
                                    .expect("Infallible serialisation of supply enum")
                                    .as_str(),
                            ),
                        )
                        .then(
                            ext_self::ext(env::current_account_id())
                                .with_static_gas(Self::AFTER_SUPPLY_ENSURE_GAS)
                                .with_unused_gas_weight(0)
                                .after_supply_1_check(op_id, index, U128(to_supply)),
                        ),
                );
            }
            // Plan exhausted; stop and reconcile remaining in stop_and_exit
            return self.stop_and_exit::<Error>(None);
        }

        if let Some(market) = self.supply_queue.get(index) {
            let cap = self.config.get(market).map_or(0, |c| c.cap);
            let cur = self.market_supply.get(market).unwrap_or(&0);
            let room = cap.saturating_sub(*cur);
            let to_supply = room.min(remaining);

            // Emit planned step event (queue-based)
            Event::AllocationStepPlanned {
                op_id,
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
                    op_id,
                    index,
                    market: market.clone(),
                    reason: "no-room".to_string(),
                    remaining: U128(remaining),
                }
                .emit();

                self.op_state = OpState::Allocating {
                    op_id,
                    index: index + 1,
                    remaining,
                };
                return self.step_allocation();
            }
            PromiseOrValue::Promise(
                self.underlying_asset
                    .transfer_call(
                        market,
                        U128(to_supply).into(),
                        Some(
                            #[allow(clippy::expect_used, reason = "Infallible")]
                            serde_json::to_string(&templar_common::market::DepositMsg::Supply)
                                .expect("Infallible serialisation of supply enum")
                                .as_str(),
                        ),
                    )
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(Self::AFTER_SUPPLY_ENSURE_GAS)
                            .after_supply_1_check(op_id, index, U128(to_supply)),
                    ),
            )
        } else {
            self.stop_and_exit(Some("Market not found"))
        }
    }

    fn start_withdraw(
        &mut self,
        amount: u128,
        receiver: AccountId,
        owner: AccountId,
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

        self.op_state = OpState::Withdrawing {
            op_id,
            index: Default::default(),
            remaining,
            receiver,
            collected,
            owner,
            escrow_shares,
        };
        self.step_withdraw()
    }

    fn step_withdraw(&mut self) -> PromiseOrValue<()> {
        let (op_id, index, remaining, receiver, collected, owner, escrow_shares) = match &self
            .op_state
        {
            OpState::Withdrawing {
                op_id,
                index,
                remaining,
                receiver,
                collected,
                owner,
                escrow_shares,
            } => (
                *op_id,
                *index,
                *remaining,
                receiver.clone(),
                *collected,
                owner.clone(),
                *escrow_shares,
            ),
            _ => return self.stop_and_exit(Some(&Error::NotWithdrawing(self.op_state.clone()))),
        };
        if remaining == 0 {
            self.op_state = OpState::Payout {
                op_id,
                receiver: receiver.clone(),
                amount: collected,
                owner: owner.clone(),
                escrow_shares,
                burn_shares: escrow_shares,
            };
            return PromiseOrValue::Promise(
                self.underlying_asset
                    .transfer(receiver.clone(), U128(collected).into())
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(Self::AFTER_SEND_TO_USER_GAS)
                            .after_send_to_user(op_id, receiver, U128(collected)),
                    ),
            );
        }
        if let Some(market) = self.withdraw_queue.get(index) {
            let have = self.market_supply.get(market).unwrap_or(&0);
            let to_request = have.min(&remaining);
            if to_request == &0 {
                self.op_state = OpState::Withdrawing {
                    op_id,
                    index: index + 1,
                    remaining,
                    receiver,
                    collected,
                    owner,
                    escrow_shares,
                };
                env::log_str(&format!(
                    "Skipping withdrawal for market {market} (have {have}, remaining {remaining})"
                ));
                return self.step_withdraw();
            }
            PromiseOrValue::Promise(
                templar_common::market::ext_market::ext(market.clone())
                    // FIXME: incorrect
                    .with_static_gas(Gas::from_tgas(10))
                    .create_supply_withdrawal_request(BorrowAssetAmount::from(U128(*to_request)))
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(Self::AFTER_CREATE_WITHDRAW_REQ_GAS)
                            .after_create_withdraw_req(op_id, index, U128(*to_request)),
                    ),
            )
        } else {
            self.pay_collected(op_id, remaining, receiver, collected, owner, escrow_shares)
        }
    }

    //  If we collected something, pay it out now and burn proportional shares or pay directly from idle balance
    //  TODO: should directly check idle balance first?
    //  TODO: unit test me
    fn pay_collected(
        &mut self,
        op_id: u64,
        remaining: u128,
        receiver: AccountId,
        collected: u128,
        owner: AccountId,
        escrow_shares: u128,
    ) -> PromiseOrValue<()> {
        if collected > 0 {
            let requested = collected.saturating_add(remaining);
            let burn_shares = mul_div_floor(escrow_shares, collected, requested.max(1));
            self.op_state = OpState::Payout {
                op_id,
                receiver: receiver.clone(),
                amount: collected,
                owner: owner.clone(),
                escrow_shares,
                burn_shares,
            };
            PromiseOrValue::Promise(
                self.underlying_asset
                    .transfer(receiver.clone(), U128(collected).into())
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(Self::AFTER_SEND_TO_USER_GAS)
                            .after_send_to_user(op_id, receiver, U128(collected)),
                    ),
            )
        } else {
            self.stop_and_exit(Some(&Error::InsufficientLiquidity))
        }
    }
}
#[cfg(test)]
mod tests;
