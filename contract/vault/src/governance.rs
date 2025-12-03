use templar_common::vault::MAX_QUEUE_LEN;

use super::*;
use near_sdk::AccountIdRef;
use near_sdk_contract_tools::ft::nep141::TransferError;
use std::collections::VecDeque;
use templar_common::{
    panic_with_message,
    vault::{PendingValue, Restrictions},
};

#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelockKind {
    Guardian,
    Sentinel,
    Config,
    Cap,
    MarketRemoval,
}

#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelockedAction {
    /// Change the guardian to `new_guardian`.
    GuardianChange { account: AccountId },
    /// Change the sentinel to `new_sentinel`.
    SentinelChange { account: AccountId },
    /// Change the governance timelock configuration to `new_timelock_ns`.
    /// If `kind` is `None`, all timelock types are updated; if `Some`, only the selected type.
    TimelockConfigChange {
        kind: Option<TimelockKind>,
        new_timelock_ns: U64,
    },
    /// Increase the cap for a given `market` to `new_cap`.
    CapChange { market: AccountId, new_cap: U128 },
    /// Mark a `market` for removal (timestamp is still stored on the config).
    MarketRemoval { market: AccountId },
}

#[near(serializers = [borsh])]
pub struct Timelocks {
    guardian_ns: TimestampNs,
    sentinel_ns: TimestampNs,
    pub timelock_config_ns: TimestampNs,
    cap_ns: TimestampNs,
    market_removal_ns: TimestampNs,
    pending_actions: VecDeque<PendingValue<TimelockedAction>>,
}

impl Timelocks {
    pub fn new(
        guardian_ns: TimestampNs,
        sentinel_ns: TimestampNs,
        timelock_config_ns: TimestampNs,
        cap_ns: TimestampNs,
        market_removal_ns: TimestampNs,
    ) -> Self {
        Self {
            guardian_ns,
            sentinel_ns,
            timelock_config_ns,
            cap_ns,
            market_removal_ns,
            pending_actions: VecDeque::new(),
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending_actions.len()
    }

    pub fn has_pending(&self) -> bool {
        !self.pending_actions.is_empty()
    }

    fn seek_pending_timelock(
        &self,
        find_fn: impl Fn(&TimelockedAction) -> bool,
    ) -> Option<(usize, &PendingValue<TimelockedAction>)> {
        self.pending_actions
            .iter()
            .enumerate()
            .find(|(_, entry)| find_fn(&entry.value))
    }
}

#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct Abdicator {
    map: HashMap<String, bool>,
}

impl Default for Abdicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Abdicator {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn is_abdicated(&self, method_name: &str) -> bool {
        *self.map.get(method_name).unwrap_or(&false)
    }

    pub fn abdicate(&mut self, method_name: &str) {
        self.map.insert(method_name.to_string(), true);
        Event::Abdicated {
            method_name: method_name.to_string(),
        }
        .emit();
    }

    pub fn require_not_abdicated(a: &Self, method_name: &str) {
        if a.is_abdicated(method_name) {
            templar_common::panic_with_message(&format!("abdicated {method_name}"));
        }
    }
}

#[near(serializers = [borsh])]
#[derive(Default)]
pub struct Gate {
    /// Internal flag to bypass transfer gates for trusted internal flows
    /// (e.g. escrow/redemption settlement).
    bypass_share_transfer_gates: bool,
    // restrictions currntly in the vault
    pub(crate) restrictions: Option<Restrictions>,
}

impl Gate {
    pub fn new(restrictions: Option<Restrictions>) -> Self {
        Self {
            restrictions,
            bypass_share_transfer_gates: false,
        }
    }

    pub(crate) fn enforce_policy(&self, account: &AccountIdRef) {
        if let Some(restrictions) = &self.restrictions {
            if let Some(reason) = restrictions.is_restricted(account) {
                templar_common::panic_with_message(&format!(
                    "Account {account} is restricted: {reason:?}"
                ));
            }
        }
    }

    // Common gate for NEP-141 share transfers (ft_transfer/ft_transfer_call).
    /// Blocks transfers when globally paused or when sending to a blocked recipient
    /// or a known market contract.
    fn enforce_share_transfer_gates(
        &self,
        markets: &BTreeMap<AccountId, MarketRecord>,
        t: &Nep141Transfer,
    ) {
        if self.bypass_share_transfer_gates {
            return;
        }

        require!(
            !markets.contains_key(t.receiver_id.as_ref()),
            "Cannot transfer shares to a market contract that is managed by the vault"
        );

        self.enforce_policy(t.sender_id.as_ref());
        self.enforce_policy(t.receiver_id.as_ref());
    }

    /// Bypass share transfer gates for a given transfer.
    /// Utilises a closure to handle errors, allowing for custom error handling.
    pub fn bypass_transfer_with(
        c: &mut Contract,
        t: &Nep141Transfer,
        on_err: impl FnOnce(TransferError),
    ) {
        c.gate.bypass_share_transfer_gates = true;
        c.transfer(t).unwrap_or_else(on_err);
        c.gate.bypass_share_transfer_gates = false;
    }

    /// Bypass share transfer gates for a given transfer. Panics on error.
    ///
    /// # Panics
    /// Panics if the transfer fails.
    pub fn bypass_transfer(c: &mut Contract, t: &Nep141Transfer) {
        // Escrow shares into the vault; bypass transfer gates for this internal flow.
        c.gate.bypass_share_transfer_gates = true;

        Gate::bypass_transfer_with(c, t, |e| panic_with_message(&e.to_string()));
        c.gate.bypass_share_transfer_gates = false;
    }
}

impl near_sdk_contract_tools::hook::Hook<Self, Nep141Transfer<'_>> for Contract {
    fn hook<R>(
        contract: &mut Self,
        transfer: &Nep141Transfer<'_>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        // Gate all NEP-141 share transfers.
        contract
            .gate
            .enforce_share_transfer_gates(&contract.markets, transfer);
        f(contract)
    }
}

#[near]
impl Contract {
    /// Sets the Curator account. Also grants/removes the Allocator role accordingly.
    pub fn set_curator(&mut self, account: AccountId) {
        Self::require_owner();
        Abdicator::require_not_abdicated(&self.abdicator, "set_curator");
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
        Abdicator::require_not_abdicated(&self.abdicator, "set_is_allocator");
        if allowed {
            Self::add_role(self, &account, &Role::Allocator);
        } else {
            self.remove_role(&account, &Role::Allocator);
        }
        Event::AllocatorRoleSet { account, allowed }.emit();
    }

    /// Sets the recipient account for skimmed tokens.
    pub fn set_skim_recipient(&mut self, account: AccountId) {
        Self::require_owner();
        Abdicator::require_not_abdicated(&self.abdicator, "set_skim_recipient");
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
    pub fn set_fee_recipient(&mut self, account: AccountId) {
        Self::require_owner();
        Abdicator::require_not_abdicated(&self.abdicator, "set_fee_recipient");
        require!(account != self.fee_recipient, "Already set to this address");

        if self.performance_fee != wad::Wad::zero() {
            self.internal_accrue_fee();
        }
        Event::FeeRecipientSet {
            account: account.clone(),
        }
        .emit();
        if self.storage_balance_of(account.clone()).is_none() {
            self.storage_deposit(Some(account.clone()), Some(true));
        }

        self.fee_recipient = account;
    }

    /// Sets the performance fee as a `WAD`. Accrues fees at the old rate first.
    pub fn set_performance_fee(&mut self, fee: Wad) {
        Self::require_owner();
        Abdicator::require_not_abdicated(&self.abdicator, "set_performance_fee");

        require!(fee != self.performance_fee, "Fee already set to this value");
        require!(fee <= Wad::from(MAX_FEE_WAD), "fee too high");

        self.internal_accrue_fee();

        self.performance_fee = fee;
        Event::PerformanceFeeSet {
            fee: U128(u128::from(fee)),
        }
        .emit();
    }

    pub fn submit_change(&mut self, action: TimelockedAction) {
        let should_schedule_timelock = self.decide_should_queue(&action);

        if should_schedule_timelock {
            self.schedule_timelock(&action);
        } else {
            self.apply_immediately(&action);
        }
    }

    /// Proposes a new Guardian. If a Guardian already exists, starts a timelock; otherwise sets immediately.
    pub fn submit_guardian(&mut self, account: AccountId) {
        let tl = TimelockedAction::GuardianChange { account };
        self.submit_change(tl);
    }

    /// Accepts the pending Guardian change after the timelock has elapsed.
    pub fn accept_guardian(&mut self) {
        Self::require_owner();

        if let Some(action) =
            self.take_timelock(|a| matches!(a, TimelockedAction::GuardianChange { .. }))
        {
            self.apply_immediately(&action);
        } else {
            panic!("No pending change");
        }
    }

    /// Revokes any pending Guardian change.
    pub fn revoke_pending_guardian(&mut self) {
        Self::assert_guardian_or_sentinel_or_owner();

        if self.revoke_timelocks(|a| matches!(a, TimelockedAction::GuardianChange { .. })) {
            Event::PendingTimelockRevoked.emit();
        }
    }

    /// Proposes a new Sentinel. If a Sentinel already exists, starts a timelock; otherwise sets immediately.
    pub fn submit_sentinel(&mut self, account: AccountId) {
        let tl = TimelockedAction::SentinelChange { account };
        self.submit_change(tl);
    }

    /// Accepts the pending Sentinel change after the timelock has elapsed.
    pub fn accept_sentinel(&mut self) {
        Self::require_owner();

        if let Some(action) =
            self.take_timelock(|a| matches!(a, TimelockedAction::SentinelChange { .. }))
        {
            self.apply_immediately(&action);
        } else {
            panic!("No pending change");
        }
    }

    /// Revokes any pending Sentinel change.
    pub fn revoke_pending_sentinel(&mut self) {
        Self::assert_guardian_or_sentinel_or_owner();

        if self.revoke_timelocks(|a| matches!(a, TimelockedAction::SentinelChange { .. })) {
            Event::PendingTimelockRevoked.emit();
        }
    }

    /* ----- Timelocks / Pending ----- */

    /// Proposes a new governance timelock in nanoseconds.
    /// If increasing, applies immediately; if decreasing, starts a timelock equal to the current duration.
    ///
    /// If `kind` is:
    /// - `None`: update all governance timelock types to `new_timelock_ns`.
    /// - `Some`: update only the corresponding timelock type.
    pub fn submit_timelock(&mut self, new_timelock_ns: U64, kind: Option<TimelockKind>) {
        let tl = TimelockedAction::TimelockConfigChange {
            kind,
            new_timelock_ns,
        };
        self.submit_change(tl);
    }

    /// Accepts a pending timelock change after it becomes valid.
    pub fn accept_timelock(&mut self) {
        Self::require_owner();

        if let Some(action) =
            self.take_timelock(|a| matches!(a, TimelockedAction::TimelockConfigChange { .. }))
        {
            self.apply_immediately(&action);
        } else {
            panic_with_message("No pending change");
        }
    }

    /// Revokes any pending timelock change.
    pub fn revoke_pending_timelock(&mut self) {
        Self::assert_guardian_or_sentinel_or_owner();
        if self.revoke_timelocks(|a| matches!(a, TimelockedAction::TimelockConfigChange { .. })) {
            Event::PendingTimelockRevoked.emit();
        }
    }

    /// Submits a change to a market's supply cap.
    /// Decreases apply immediately; increases are subject to the governance timelock.
    ///
    /// If the market does not exist, it will be created when the timelock is executed.
    pub fn submit_cap(&mut self, market: AccountId, new_cap: U128) {
        self.submit_change(TimelockedAction::CapChange { market, new_cap });
    }

    /// Accepts a pending cap increase for `market` once the timelock has elapsed.
    ///
    /// # Panics
    /// If there is no pending cap change for this market.
    pub fn accept_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        self.ensure_idle();

        if let Some(action) = self.take_timelock(
            |a| matches!(a, TimelockedAction::CapChange { market: mkt, .. } if mkt == &market),
        ) {
            self.apply_immediately(&action);
        } else {
            panic_with_message("No pending cap change for this market");
        }
    }

    /// Revokes any pending cap change for `market`.
    pub fn revoke_pending_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_sentinel_or_owner();

        if self.revoke_timelocks(
            |a| matches!(a, TimelockedAction::CapChange { market: mkt, .. } if mkt == &market),
        ) {
            Event::SupplyCapRaiseRevoked {
                market: market.clone(),
            }
            .emit();
        }
    }

    /// To remove a market entirely, the curator:
    /// - first sets its cap to 0 (disabling new deposits)
    /// - then calls submit_market_removal.
    /// This starts a timelock (using the vault’s timelock),
    /// after which the market may be disabled/removed once funds have been withdrawn, if any.
    /// Begins the process to remove `market`.
    /// Requires cap == 0 and no pending cap changes; starts a timelock.
    pub fn submit_market_removal(&mut self, market: AccountId) {
        self.submit_change(TimelockedAction::MarketRemoval { market });
    }

    /// Accepts a pending market removal for `market` after the timelock has elapsed.
    pub fn accept_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();

        if let Some(action) = self.take_timelock(
            |a| matches!(a, TimelockedAction::MarketRemoval { market: mkt } if mkt == &market),
        ) {
            self.apply_immediately(&action);
        } else {
            panic_with_message("No pending market removal for this market");
        }
    }

    /// Revokes a pending market removal for `market`.
    pub fn revoke_pending_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_sentinel_or_owner();

        self.revoke_timelocks(
            |a| matches!(a, TimelockedAction::MarketRemoval { market: mkt } if mkt == &market),
        );
        if let Some(m) = self.markets.get_mut(&market) {
            m.cfg.removable_at = 0;
        }
        Event::MarketRemovalRevoked { market }.emit();
    }

    /// Sets the ordered supply queue.
    /// Rejects duplicates and markets without a positive cap. Requires the vault to be idle.
    #[payable]
    pub fn set_supply_queue(&mut self, markets: Vec<AccountId>) {
        Self::assert_allocator();
        Abdicator::require_not_abdicated(&self.abdicator, "set_supply_queue");
        self.ensure_idle();
        require!(markets.len() <= MAX_QUEUE_LEN, "too long");

        // Invariant: supply_queue has no duplicates
        let mut seen = HashSet::new();
        for m in &markets {
            if !seen.insert(m.clone()) {
                panic_with_message(&format!("Duplicate market {m}"));
            }
        }

        // Validate all markets are authorized (cap > 0) before charging storage
        for m in &markets {
            let cap = self.markets.get(m).map_or(0, |r| r.cfg.cap.into());
            require!(cap > 0, "unauthorized market");
        }

        // Compute and require storage for additions (no refunds for removals in this pass)
        let current: BTreeSet<AccountId> = self.supply_queue.iter().cloned().collect();
        let required_yocto = storage_management::yocto_for_queue_additions(&current, &markets);
        let _ = require_attached_at_least(required_yocto, "supply queue update");

        self.supply_queue.clear();

        for m in &markets {
            self.supply_queue.insert(m.clone());
        }
    }

    /// Permanently disables a governance method by name.
    pub fn abdicate(&mut self, method_name: String) {
        Self::assert_curator_or_owner();
        self.abdicator.abdicate(&method_name);
    }

    /// Sets the restrictions for the vault.
    pub fn set_restrictions(&mut self, restrictions: Option<Restrictions>) {
        Self::assert_guardian_or_owner();
        env::log_str(&format!("Restrictions set to {restrictions:?}"));
        self.gate.restrictions = restrictions;
    }

    pub fn get_restrictions(&self) -> Option<Restrictions> {
        self.gate.restrictions.clone()
    }
}

impl Contract {
    #[allow(clippy::too_many_lines)]
    fn decide_should_queue(&self, action: &TimelockedAction) -> bool {
        match action {
            // Submit a timelocked governance change if there is already a guardian
            TimelockedAction::GuardianChange { .. } => {
                Self::require_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_guardian");

                Self::with_members_of(&Role::Guardian, |members| {
                    require!(
                        members.len() < 2,
                        "Invariant violation: Cannot have more than one Guardian"
                    );
                    !members.is_empty()
                })
            }
            TimelockedAction::SentinelChange { .. } => {
                Self::require_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_sentinel");

                Self::with_members_of(&Role::Sentinel, |members| {
                    require!(
                        members.len() < 2,
                        "Invariant violation: Cannot have more than one Sentinel"
                    );
                    !members.is_empty()
                })
            }
            // Submit a timelocked governance change if the selected timelock is to be smaller than the current value
            TimelockedAction::TimelockConfigChange {
                kind,
                new_timelock_ns,
            } => {
                Self::assert_guardian_or_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_timelock");

                let new = new_timelock_ns.0;
                let current = match kind {
                    None | Some(TimelockKind::Config) => {
                        self.governance_timelocks.timelock_config_ns
                    }
                    Some(TimelockKind::Guardian) => self.governance_timelocks.guardian_ns,
                    Some(TimelockKind::Sentinel) => self.governance_timelocks.sentinel_ns,
                    Some(TimelockKind::Cap) => self.governance_timelocks.cap_ns,
                    Some(TimelockKind::MarketRemoval) => {
                        self.governance_timelocks.market_removal_ns
                    }
                };

                require!(new != current, "Already set to this value");
                require!(
                    (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&new),
                    "Timelock out of bounds"
                );
                new < current
            }
            // Submit a timelocked governance change if the cap is greater than the current cap or there is a new market to be made
            TimelockedAction::CapChange { market, new_cap } => {
                Self::assert_curator_or_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_cap");
                self.ensure_idle();

                let cfg = self.markets.get(market).map(|m| &m.cfg);

                if let Some(cfg) = cfg {
                    require!(
                        self.governance_timelocks
                            .seek_pending_timelock(|p| matches!(
                                p,
                                TimelockedAction::MarketRemoval { market: m } if m == market
                            ))
                            .is_none(),
                        "Market removal pending, cannot change cap"
                    );
                    require!(
                        cfg.removable_at == 0,
                        "Market removal pending, cannot change cap"
                    );
                    require!(new_cap != &cfg.cap, "New cap is same as current");
                    new_cap > &cfg.cap
                } else {
                    true
                }
            }
            // Submit a timelocked governance change to remove a market
            TimelockedAction::MarketRemoval { market } => {
                Self::assert_curator_or_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_market_removal");

                let r = self
                    .markets
                    .get(market)
                    .unwrap_or_else(|| panic_with_message(&format!("Unknown market: {market}")));

                require!(
                    self.governance_timelocks
                        .seek_pending_timelock(|p| matches!(
                            p,
                            TimelockedAction::MarketRemoval { market: m } if m == market
                        ))
                        .is_none(),
                    "Removal already pending for this market"
                );
                require!(
                    r.cfg.removable_at == 0,
                    "Removal already accepted for this market"
                );
                require!(
                    r.cfg.cap.0 == 0,
                    "Cannot remove market with non-zero cap (disable deposits first)"
                );
                require!(r.cfg.enabled, "Market not enabled or already removed");
                require!(
                    self.governance_timelocks
                        .seek_pending_timelock(|p| matches!(
                            p,
                            TimelockedAction::CapChange { market: m, .. } if m == market
                        ))
                        .is_none(),
                    "Cap change pending for this market"
                );
                r.principal > 0
            }
        }
    }

    fn apply_immediately(&mut self, action: &TimelockedAction) {
        match action {
            TimelockedAction::GuardianChange { account } => {
                Self::with_members_of_mut(&Role::Guardian, |members| {
                    members.clear();
                    members.insert(account);
                });
                Event::GuardianSet {
                    account: account.clone(),
                }
                .emit();
            }
            TimelockedAction::SentinelChange { account } => {
                Self::with_members_of_mut(&Role::Sentinel, |members| {
                    members.clear();
                    members.insert(account);
                });
                Event::SentinelSet {
                    account: account.clone(),
                }
                .emit();
            }
            TimelockedAction::TimelockConfigChange {
                kind,
                new_timelock_ns,
            } => {
                let new_ns = new_timelock_ns.0;
                match kind {
                    None => {
                        self.governance_timelocks.guardian_ns = new_ns;
                        self.governance_timelocks.sentinel_ns = new_ns;
                        self.governance_timelocks.timelock_config_ns = new_ns;
                        self.governance_timelocks.market_removal_ns = new_ns;
                        self.governance_timelocks.cap_ns = new_ns;
                    }
                    Some(TimelockKind::Guardian) => {
                        self.governance_timelocks.guardian_ns = new_ns;
                    }
                    Some(TimelockKind::Sentinel) => {
                        self.governance_timelocks.sentinel_ns = new_ns;
                    }
                    Some(TimelockKind::Config) => {
                        self.governance_timelocks.timelock_config_ns = new_ns;
                    }
                    Some(TimelockKind::Cap) => {
                        self.governance_timelocks.cap_ns = new_ns;
                    }
                    Some(TimelockKind::MarketRemoval) => {
                        self.governance_timelocks.market_removal_ns = new_ns;
                    }
                }
                Event::TimelockSet {
                    seconds: *new_timelock_ns,
                }
                .emit();
            }
            TimelockedAction::CapChange { market, new_cap } => {
                let mkt = match self.markets.get_mut(market) {
                    None => {
                        self.markets.insert(market.clone(), MarketRecord::default());
                        Event::MarketCreated {
                            market: market.clone(),
                        }
                        .emit();
                        self.markets
                            .get_mut(market)
                            .unwrap_or_else(|| panic_with_message("Config not found"))
                    }
                    Some(m) => m,
                };

                let was_enabled = mkt.cfg.enabled;

                if new_cap.0 > 0 {
                    if !was_enabled {
                        mkt.cfg.enabled = true;
                        Event::MarketEnabled {
                            market: market.clone(),
                        }
                        .emit();
                    }
                    mkt.cfg.removable_at = 0;
                }

                Event::SupplyCapSet {
                    market: market.clone(),
                    new_cap: *new_cap,
                }
                .emit();
                mkt.cfg.cap = *new_cap;
            }
            TimelockedAction::MarketRemoval { market } => {
                let rec = self
                    .markets
                    .get_mut(market)
                    .unwrap_or_else(|| panic_with_message(&format!("Unknown market: {market}")));

                rec.cfg.removable_at = env::block_timestamp();
                Event::MarketRemovalSubmitted {
                    market: market.clone(),
                    removable_at: rec.cfg.removable_at.into(),
                }
                .emit();
            }
        }
    }

    /// Schedule a new timelocked governance action.
    ///
    /// Fails if an identical action is already pending.
    fn schedule_timelock(&mut self, action: &TimelockedAction) {
        let cur = match action {
            TimelockedAction::GuardianChange { .. } => self.governance_timelocks.guardian_ns,
            TimelockedAction::SentinelChange { .. } => self.governance_timelocks.sentinel_ns,
            TimelockedAction::TimelockConfigChange { kind, .. } => match kind {
                None | Some(TimelockKind::Config) => self.governance_timelocks.timelock_config_ns,
                Some(TimelockKind::Guardian) => self.governance_timelocks.guardian_ns,
                Some(TimelockKind::Sentinel) => self.governance_timelocks.sentinel_ns,
                Some(TimelockKind::Cap) => self.governance_timelocks.cap_ns,
                Some(TimelockKind::MarketRemoval) => self.governance_timelocks.market_removal_ns,
            },
            TimelockedAction::CapChange { .. } => self.governance_timelocks.cap_ns,
            TimelockedAction::MarketRemoval { .. } => self.governance_timelocks.market_removal_ns,
        };

        require!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&cur),
            "Timelock duration out of bounds"
        );

        require!(
            self.governance_timelocks
                .seek_pending_timelock(|a| a == action)
                .is_none(),
            "Change already pending for this action and arguments"
        );

        let valid_at_ns = env::block_timestamp().saturating_add(cur);

        self.governance_timelocks
            .pending_actions
            .push_back(PendingValue {
                value: action.clone(),
                valid_at_ns,
            });

        Event::TimelockChangeSubmitted {
            valid_at_ns: valid_at_ns.into(),
        }
        .emit();
    }

    /// Find and consume the first pending action that matches `pred`.
    ///
    /// Returns `None` if no such action exists, or panics if the timelock hasn't elapsed yet.
    fn take_timelock(
        &mut self,
        find_fn: impl Fn(&TimelockedAction) -> bool,
    ) -> Option<TimelockedAction> {
        let (i, entry) = self.governance_timelocks.seek_pending_timelock(find_fn)?;
        entry.verify();
        let action = entry.value.clone();
        self.governance_timelocks.pending_actions.remove(i);
        Some(action)
    }

    /// Remove all pending actions that match `pred`.
    /// Returns `true` if at least one action was removed.
    fn revoke_timelocks(&mut self, pred: impl Fn(&TimelockedAction) -> bool) -> bool {
        let mut removed_any = false;
        self.governance_timelocks.pending_actions.retain(|entry| {
            let keep = !pred(&entry.value);
            if !keep {
                removed_any = true;
            }
            keep
        });
        removed_any
    }
}
