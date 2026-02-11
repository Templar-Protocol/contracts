use templar_common::vault::{
    wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD},
    CapGroupUpdate, CapGroupUpdateKey, TimelockKind, TimestampNs, MAX_QUEUE_LEN,
};

use super::*;
use crate::auth::AuthPattern;
use near_sdk::AccountIdRef;
use near_sdk_contract_tools::ft::nep141::TransferError;
use near_sdk_contract_tools::ft::Nep141Transfer;
use std::collections::VecDeque;
use templar_common::{panic_with_message, vault::Restrictions};
use templar_curator_primitives::boundary::{
    cap_change_error_message, fee_change_error_message, membership_change_error_message,
    relative_cap_change_error_message, timelock_config_error_message,
};
use templar_curator_primitives::governance as shared_gov;
use templar_curator_primitives::governance::PendingValue;

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
    /// Update fee rates and recipients.
    FeesChange { fees: Fees<U128> },
    /// Update account restrictions / gating policy.
    RestrictionsChange { restrictions: Option<Restrictions> },
    /// Increase the cap for a given `market` to `new_cap`.
    CapChange { market: AccountId, new_cap: U128 },
    /// Increase the cap for a correlated-risk cap group.
    CapGroupChange {
        cap_group: CapGroupId,
        new_cap: U128,
    },
    /// Change the relative cap (fraction of total vault assets) for a cap group.
    CapGroupRelativeCapChange {
        cap_group: CapGroupId,
        new_relative_cap: U128,
    },
    /// Assign (or remove) a market to/from a cap group.
    CapGroupMembership {
        market: MarketId,
        cap_group: Option<CapGroupId>,
    },
    /// Mark a `market` for removal (timestamp is still stored on the config).
    MarketRemoval { market: AccountId },
}

fn to_shared_restrictions(
    restrictions: &Option<Restrictions>,
) -> Option<shared_gov::Restrictions<AccountId>> {
    match restrictions {
        None => None,
        Some(Restrictions::Paused) => Some(shared_gov::Restrictions::Paused),
        Some(Restrictions::Blacklist(list)) => Some(shared_gov::Restrictions::Blacklist(
            list.iter().cloned().collect(),
        )),
        Some(Restrictions::Whitelist(list)) => Some(shared_gov::Restrictions::Whitelist(
            list.iter().cloned().collect(),
        )),
    }
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

impl From<TimestampNs> for Timelocks {
    fn from(ns: TimestampNs) -> Self {
        Self {
            guardian_ns: ns,
            sentinel_ns: ns,
            timelock_config_ns: ns,
            cap_ns: ns,
            market_removal_ns: ns,
            pending_actions: VecDeque::new(),
        }
    }
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

    pub fn pending_actions(&self) -> Vec<PendingValue<TimelockedAction>> {
        self.pending_actions.iter().cloned().collect()
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
    // restrictions currently in the vault
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
        market_ids: &BTreeMap<AccountId, MarketId>,
        t: &Nep141Transfer,
    ) {
        if self.bypass_share_transfer_gates {
            return;
        }

        require!(
            !market_ids.contains_key(t.receiver_id.as_ref()),
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
        let previous = c.gate.bypass_share_transfer_gates;
        c.gate.bypass_share_transfer_gates = true;

        let result = c.transfer(t);

        c.gate.bypass_share_transfer_gates = previous;

        result.unwrap_or_else(on_err);
    }

    /// Bypass share transfer gates for a given transfer. Panics on error.
    ///
    /// # Panics
    /// Panics if the transfer fails.
    pub fn bypass_transfer(c: &mut Contract, t: &Nep141Transfer) {
        Gate::bypass_transfer_with(c, t, |e| panic_with_message(&e.to_string()));
    }
}

impl near_sdk_contract_tools::hook::Hook<Self, Nep141Transfer<'_>> for Contract {
    fn hook<R>(
        contract: &mut Self,
        transfer: &Nep141Transfer<'_>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        contract
            .gate
            .enforce_share_transfer_gates(&contract.market_ids, transfer);
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

    /// Sets both performance and management fee rates and recipients atomically.
    ///
    /// Timelock semantics (Morpho-like):
    /// - Fee decreases apply immediately.
    /// - Fee increases and any recipient change are subject to the governance timelock.
    pub fn set_fees(&mut self, fees: Fees<U128>) {
        self.submit_change(TimelockedAction::FeesChange { fees });
    }

    /// Accepts a pending fee change after the timelock has elapsed.
    pub fn accept_fees(&mut self) {
        Self::require_owner();

        if let Some(action) =
            self.take_timelock(|a| matches!(a, TimelockedAction::FeesChange { .. }))
        {
            self.apply_immediately(&action);
        } else {
            panic_with_message("No pending fee change");
        }
    }

    /// Revokes any pending fee change.
    pub fn revoke_pending_fees(&mut self) {
        AuthPattern::GuardianOrSentinelOrOwner.require();

        if self.revoke_timelocks(|a| matches!(a, TimelockedAction::FeesChange { .. })) {
            Event::FeesChangeRevoked.emit();
        }
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
            panic_with_message("No pending change");
        }
    }

    /// Revokes any pending Guardian change.
    pub fn revoke_pending_guardian(&mut self) {
        AuthPattern::GuardianOrSentinelOrOwner.require();

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
            panic_with_message("No pending change");
        }
    }

    /// Revokes any pending Sentinel change.
    pub fn revoke_pending_sentinel(&mut self) {
        AuthPattern::GuardianOrSentinelOrOwner.require();

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
        AuthPattern::GuardianOrSentinelOrOwner.require();
        if self.revoke_timelocks(|a| matches!(a, TimelockedAction::TimelockConfigChange { .. })) {
            Event::PendingTimelockRevoked.emit();
        }
    }

    /// Submits a change to a market's supply cap.
    /// Decreases apply immediately; increases are subject to the governance timelock.
    ///
    /// If the market does not exist, it will be created when the timelock is executed.
    pub fn submit_cap(&mut self, market: AccountId, new_cap: U128) {
        let _ = self.ensure_market_record(&market);
        self.submit_change(TimelockedAction::CapChange { market, new_cap });
    }

    /// Accepts a pending cap increase for `market` once the timelock has elapsed.
    ///
    /// # Panics
    /// If there is no pending cap change for this market.
    pub fn accept_cap(&mut self, market: AccountId) {
        AuthPattern::CuratorOrOwner.require();
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
        AuthPattern::CuratorOrSentinelOrOwner.require();

        let market_id = self.market_id_of_or_panic(&market);

        if self.revoke_timelocks(
            |a| matches!(a, TimelockedAction::CapChange { market: mkt, .. } if mkt == &market),
        ) {
            Event::SupplyCapRaiseRevoked { market: market_id }.emit();
        }
    }

    /// Submits a cap-group governance update.
    ///
    /// Consolidates the cap-group surface area across:
    /// - absolute cap
    /// - relative cap
    /// - market ↔ group membership
    pub fn submit_cap_group_update(&mut self, update: CapGroupUpdate) {
        let action = match update {
            CapGroupUpdate::SetCap { cap_group, new_cap } => {
                TimelockedAction::CapGroupChange { cap_group, new_cap }
            }
            CapGroupUpdate::SetRelativeCap {
                cap_group,
                new_relative_cap,
            } => TimelockedAction::CapGroupRelativeCapChange {
                cap_group,
                new_relative_cap,
            },
            CapGroupUpdate::SetMarketCapGroup { market, cap_group } => {
                TimelockedAction::CapGroupMembership { market, cap_group }
            }
        };

        self.submit_change(action);
    }

    /// Accepts a pending cap-group update once the timelock has elapsed.
    ///
    /// # Panics
    /// If there is no matching pending cap-group update.
    pub fn accept_cap_group_update(&mut self, update: CapGroupUpdateKey) {
        AuthPattern::CuratorOrOwner.require();
        self.ensure_idle();

        let action = match update {
            CapGroupUpdateKey::SetCap { cap_group } => self
                .take_timelock(|a| {
                    matches!(
                        a,
                        TimelockedAction::CapGroupChange {
                            cap_group: pending,
                            ..
                        } if pending == &cap_group
                    )
                })
                .unwrap_or_else(|| panic_with_message("No pending cap group change for this id")),
            CapGroupUpdateKey::SetRelativeCap { cap_group } => self
                .take_timelock(|a| {
                    matches!(
                        a,
                        TimelockedAction::CapGroupRelativeCapChange {
                            cap_group: pending,
                            ..
                        } if pending == &cap_group
                    )
                })
                .unwrap_or_else(|| {
                    panic_with_message("No pending cap group relative cap change for this id")
                }),
            CapGroupUpdateKey::SetMarketCapGroup { market } => self
                .take_timelock(|a| {
                    matches!(
                        a,
                        TimelockedAction::CapGroupMembership { market: pending, .. }
                            if pending == &market
                    )
                })
                .unwrap_or_else(|| {
                    panic_with_message("No pending cap group membership change for this market")
                }),
        };

        self.apply_immediately(&action);
    }

    /// Revokes a pending cap-group update.
    pub fn revoke_pending_cap_group_update(&mut self, update: CapGroupUpdateKey) {
        AuthPattern::CuratorOrOwner.require();

        match update {
            CapGroupUpdateKey::SetCap { cap_group } => {
                if self.revoke_timelocks(|a| {
                    matches!(
                        a,
                        TimelockedAction::CapGroupChange {
                            cap_group: pending,
                            ..
                        } if pending == &cap_group
                    )
                }) {
                    Event::CapGroupRaiseRevoked { cap_group }.emit();
                }
            }
            CapGroupUpdateKey::SetRelativeCap { cap_group } => {
                if self.revoke_timelocks(|a| {
                    matches!(
                        a,
                        TimelockedAction::CapGroupRelativeCapChange {
                            cap_group: pending,
                            ..
                        } if pending == &cap_group
                    )
                }) {
                    Event::CapGroupRelativeCapRaiseRevoked { cap_group }.emit();
                }
            }
            CapGroupUpdateKey::SetMarketCapGroup { market } => {
                if self.revoke_timelocks(|a| {
                    matches!(
                        a,
                        TimelockedAction::CapGroupMembership { market: pending, .. }
                            if pending == &market
                    )
                }) {
                    Event::CapGroupMembershipRevoked { market }.emit();
                }
            }
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
        AuthPattern::CuratorOrOwner.require();

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
        AuthPattern::CuratorOrSentinelOrOwner.require();

        let market_id = self.market_id_of_or_panic(&market);

        self.revoke_timelocks(
            |a| matches!(a, TimelockedAction::MarketRemoval { market: mkt } if mkt == &market),
        );
        if let Some(m) = self.market_record_by_id_mut(market_id) {
            m.cfg.removable_at = 0;
        }
        Event::MarketRemovalRevoked { market: market_id }.emit();
    }

    /// Sets the ordered supply queue.
    /// Rejects duplicates and markets without a positive cap. Requires the vault to be idle.
    #[payable]
    pub fn set_supply_queue(&mut self, markets: Vec<MarketId>) {
        crate::auth::require_action(crate::auth::ActionKind::BeginAllocating);
        Abdicator::require_not_abdicated(&self.abdicator, "set_supply_queue");
        self.ensure_idle();
        require!(markets.len() <= MAX_QUEUE_LEN, "too long");

        {
            use crate::convert::IntoTargetId;
            let ids: Vec<u32> = markets.iter().map(IntoTargetId::into_target_id).collect();
            if let Some(dup) =
                templar_curator_primitives::policy::target_set::find_duplicate_target_id(&ids)
            {
                use crate::convert::IntoMarketId;
                panic_with_message(&format!(
                    "Duplicate market in supply queue: {}",
                    dup.into_market_id()
                ));
            }
        }

        // Validate all markets are authorized (cap > 0) before charging storage
        for m in &markets {
            let cap = self
                .market_record_by_id(*m)
                .unwrap_or_else(|| panic_with_message(&format!("Unknown market id: {m}")))
                .cfg
                .cap
                .0;
            require!(cap > 0, "unauthorized market");
        }

        // Compute and require storage for additions (no refunds for removals in this pass)
        let current = self.supply_queue.clone();
        let required_yocto = storage_management::yocto_for_queue_additions(&current, &markets);
        let _ = require_attached_at_least(required_yocto, "supply queue update");

        self.supply_queue.clear();
        self.supply_queue.extend(markets);
    }

    /// Permanently disables a governance method by name.
    pub fn abdicate(&mut self, method_name: String) {
        AuthPattern::CuratorOrOwner.require();
        self.abdicator.abdicate(&method_name);
    }

    /// Sets the restrictions for the vault.
    ///
    /// Operational guidance:
    /// - Incident response should use `Restrictions::Paused` rather than per-account blacklisting.
    /// - `Blacklist`/`Whitelist` are governance/policy controls and are censorship-sensitive.
    ///
    /// Timelock semantics:
    /// - Tightening restrictions (including `Paused`) applies immediately.
    /// - Unpause/relax actions are subject to the governance timelock.
    pub fn set_restrictions(&mut self, restrictions: Option<Restrictions>) {
        self.submit_change(TimelockedAction::RestrictionsChange { restrictions });
    }

    /// Accepts a pending restrictions change after the timelock has elapsed.
    pub fn accept_restrictions(&mut self) {
        AuthPattern::GuardianOrOwner.require();

        if let Some(action) =
            self.take_timelock(|a| matches!(a, TimelockedAction::RestrictionsChange { .. }))
        {
            self.apply_immediately(&action);
        } else {
            panic_with_message("No pending restrictions change");
        }
    }

    /// Revokes any pending restrictions change.
    pub fn revoke_pending_restrictions(&mut self) {
        AuthPattern::GuardianOrSentinelOrOwner.require();

        if self.revoke_timelocks(|a| matches!(a, TimelockedAction::RestrictionsChange { .. })) {
            Event::RestrictionsChangeRevoked.emit();
        }
    }

    pub fn get_restrictions(&self) -> Option<Restrictions> {
        self.gate.restrictions.clone()
    }
}

impl Contract {
    fn ensure_market_record(&mut self, market: &AccountId) -> MarketId {
        if let Some(id) = self.market_id_of(market) {
            return id;
        }

        let id = self.allocate_market_id();
        self.insert_market_record(id, MarketRecord::new(market.clone()));
        Event::MarketCreated { market: id }.emit();
        id
    }

    #[allow(clippy::too_many_lines)]
    fn decide_should_queue(&self, action: &TimelockedAction) -> bool {
        match action {
            // Submit a timelocked governance change if there is already a guardian
            TimelockedAction::GuardianChange { .. } => {
                Self::require_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_guardian");

                let has_guardian = Self::with_members_of(&Role::Guardian, |members| {
                    require!(
                        members.len() < 2,
                        "Invariant violation: Cannot have more than one Guardian"
                    );
                    !members.is_empty()
                });

                shared_gov::guardian_change_decision(has_guardian).requires_timelock()
            }
            // Submit a timelocked governance change if there is already a sentinel
            TimelockedAction::SentinelChange { .. } => {
                Self::require_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_sentinel");

                let has_sentinel = Self::with_members_of(&Role::Sentinel, |members| {
                    require!(
                        members.len() < 2,
                        "Invariant violation: Cannot have more than one Sentinel"
                    );
                    !members.is_empty()
                });

                shared_gov::sentinel_change_decision(has_sentinel).requires_timelock()
            }
            // Submit a timelocked governance change if the selected timelock is to be smaller than the current value
            TimelockedAction::TimelockConfigChange {
                kind,
                new_timelock_ns,
            } => {
                AuthPattern::GuardianOrOwner.require();
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

                shared_gov::submission_requires_timelock(shared_gov::timelock_config_decision(
                    current,
                    new,
                    MIN_TIMELOCK_NS,
                    MAX_TIMELOCK_NS,
                ))
                .unwrap_or_else(|err| panic_with_message(timelock_config_error_message(err)))
            }
            TimelockedAction::FeesChange { fees } => {
                Self::require_owner();
                Abdicator::require_not_abdicated(&self.abdicator, "set_fees");

                require!(
                    !shared_gov::queue_has_pending(
                        &self.governance_timelocks.pending_actions,
                        |p| { matches!(p, TimelockedAction::FeesChange { .. }) }
                    ),
                    "Fee change already pending"
                );

                let proposed_performance_fee = Wad::from(fees.performance.fee.0);
                let proposed_management_fee = Wad::from(fees.management.fee.0);
                let proposed_max_rate = fees.max_total_assets_growth_rate.map(|r| Wad::from(r.0));

                let current = shared_gov::FeeConfig::new(
                    self.fees.performance.fee,
                    self.fees.management.fee,
                    &self.fees.performance.recipient,
                    &self.fees.management.recipient,
                    self.fees.max_total_assets_growth_rate,
                );
                let proposed = shared_gov::FeeConfig::new(
                    proposed_performance_fee,
                    proposed_management_fee,
                    &fees.performance.recipient,
                    &fees.management.recipient,
                    proposed_max_rate,
                );

                shared_gov::evaluate_fee_change(&current, &proposed)
                    .map(|decision| decision.timelocked)
                    .unwrap_or_else(|err| panic_with_message(fee_change_error_message(err)))
            }
            TimelockedAction::RestrictionsChange { restrictions } => {
                Abdicator::require_not_abdicated(&self.abdicator, "set_restrictions");
                require!(
                    restrictions != &self.gate.restrictions,
                    "No restriction changes"
                );

                let current = to_shared_restrictions(&self.gate.restrictions);
                let proposed = to_shared_restrictions(restrictions);
                let is_relaxing = shared_gov::determine_relaxed(&current, &proposed);

                if is_relaxing {
                    AuthPattern::GuardianOrOwner.require();
                    require!(
                        !shared_gov::queue_has_pending(
                            &self.governance_timelocks.pending_actions,
                            |p| { matches!(p, TimelockedAction::RestrictionsChange { .. }) }
                        ),
                        "Restrictions change already pending"
                    );
                    true
                } else {
                    // Tightening (including emergency pause) is immediate and may be done by the
                    // guardian, sentinel, or owner.
                    AuthPattern::GuardianOrSentinelOrOwner.require();
                    false
                }
            }
            // Submit a timelocked governance change if the cap is greater than the current cap or there is a new market to be made
            TimelockedAction::CapChange { market, new_cap } => {
                AuthPattern::CuratorOrOwner.require();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_cap");
                self.ensure_idle();

                let cfg = self
                    .market_id_of(market)
                    .and_then(|id| self.market_record_by_id(id).map(|m| &m.cfg));

                if let Some(cfg) = cfg {
                    require!(
                        !shared_gov::queue_has_pending(
                            &self.governance_timelocks.pending_actions,
                            |p| {
                                matches!(
                                    p,
                                    TimelockedAction::MarketRemoval { market: m } if m == market
                                )
                            }
                        ),
                        "Market removal pending, cannot change cap"
                    );
                    require!(
                        cfg.removable_at == 0,
                        "Market removal pending, cannot change cap"
                    );
                    require!(
                        !shared_gov::queue_has_pending(
                            &self.governance_timelocks.pending_actions,
                            |p| {
                                matches!(
                                    p,
                                    TimelockedAction::CapChange { market: m, .. } if m == market
                                )
                            }
                        ),
                        "Cap change already pending for this market"
                    );
                    shared_gov::submission_requires_timelock(shared_gov::cap_change_decision(
                        Some(cfg.cap.0),
                        new_cap.0,
                    ))
                    .unwrap_or_else(|err| panic_with_message(cap_change_error_message(err)))
                } else {
                    true
                }
            }
            TimelockedAction::CapGroupChange { cap_group, new_cap } => {
                AuthPattern::CuratorOrOwner.require();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_cap_group_update");
                self.ensure_idle();

                require!(
                    !shared_gov::queue_has_pending(
                        &self.governance_timelocks.pending_actions,
                        |p| {
                            matches!(
                                p,
                                TimelockedAction::CapGroupChange { cap_group: pending, .. }
                                    if pending == cap_group
                            )
                        }
                    ),
                    "Cap group change already pending"
                );

                let current = self.cap_groups.get(cap_group).map(|record| record.cap.0);
                shared_gov::submission_requires_timelock(shared_gov::cap_change_decision(
                    current, new_cap.0,
                ))
                .unwrap_or_else(|err| panic_with_message(cap_change_error_message(err)))
            }
            TimelockedAction::CapGroupRelativeCapChange {
                cap_group,
                new_relative_cap,
            } => {
                AuthPattern::CuratorOrOwner.require();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_cap_group_update");
                self.ensure_idle();

                let new_wad = Wad::from(new_relative_cap.0);

                require!(
                    !shared_gov::queue_has_pending(
                        &self.governance_timelocks.pending_actions,
                        |p| {
                            matches!(
                                p,
                                TimelockedAction::CapGroupRelativeCapChange { cap_group: pending, .. }
                                    if pending == cap_group
                            )
                        }
                    ),
                    "Cap group relative cap change already pending"
                );

                let current = self
                    .cap_groups
                    .get(cap_group)
                    .map(|record| record.relative_cap);
                shared_gov::submission_requires_timelock(shared_gov::relative_cap_change_decision(
                    current, new_wad,
                ))
                .unwrap_or_else(|err| panic_with_message(relative_cap_change_error_message(err)))
            }
            TimelockedAction::CapGroupMembership { market, cap_group } => {
                AuthPattern::CuratorOrOwner.require();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_cap_group_update");
                self.ensure_idle();

                let rec = self.market_record_by_id_or_panic(*market);

                let changed = rec.cfg.cap_group_id != *cap_group;
                let decision = shared_gov::submission_requires_timelock(
                    shared_gov::membership_change_decision(changed),
                )
                .unwrap_or_else(|err| panic_with_message(membership_change_error_message(err)));

                require!(
                    !shared_gov::queue_has_pending(
                        &self.governance_timelocks.pending_actions,
                        |p| {
                            matches!(
                                p,
                                TimelockedAction::CapGroupMembership { market: pending, .. }
                                    if pending == market
                            )
                        }
                    ),
                    "Cap group membership change already pending"
                );

                decision
            }
            // Submit a timelocked governance change to remove a market
            TimelockedAction::MarketRemoval { market } => {
                AuthPattern::CuratorOrOwner.require();
                Abdicator::require_not_abdicated(&self.abdicator, "submit_market_removal");

                let market_id = self.market_id_of_or_panic(market);
                let r = self.market_record_by_id_or_panic(market_id);

                require!(
                    !shared_gov::queue_has_pending(
                        &self.governance_timelocks.pending_actions,
                        |p| {
                            matches!(
                                p,
                                TimelockedAction::MarketRemoval { market: m } if m == market
                            )
                        }
                    ),
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
                    !shared_gov::queue_has_pending(
                        &self.governance_timelocks.pending_actions,
                        |p| {
                            matches!(
                                p,
                                TimelockedAction::CapChange { market: m, .. } if m == market
                            )
                        }
                    ),
                    "Cap change pending for this market"
                );
                shared_gov::market_removal_decision(r.principal).requires_timelock()
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
            TimelockedAction::FeesChange { fees } => {
                let performance_fee = Wad::from(fees.performance.fee.0);
                let management_fee = Wad::from(fees.management.fee.0);

                require!(
                    performance_fee <= Wad::from(MAX_PERFORMANCE_FEE_WAD),
                    "performance fee too high"
                );
                require!(
                    management_fee <= Wad::from(MAX_MANAGEMENT_FEE_WAD),
                    "management fee too high"
                );

                let max_total_assets_growth_rate =
                    fees.max_total_assets_growth_rate.map(|r| Wad::from(r.0));

                let performance_fee_changed = performance_fee != self.fees.performance.fee;
                let management_fee_changed = management_fee != self.fees.management.fee;
                let performance_recipient_changed =
                    fees.performance.recipient != self.fees.performance.recipient;
                let management_recipient_changed =
                    fees.management.recipient != self.fees.management.recipient;
                let max_rate_changed =
                    max_total_assets_growth_rate != self.fees.max_total_assets_growth_rate;

                require!(
                    performance_fee_changed
                        || management_fee_changed
                        || performance_recipient_changed
                        || management_recipient_changed
                        || max_rate_changed,
                    "No fee changes"
                );

                let has_active_fee =
                    !self.fees.performance.fee.is_zero() || !self.fees.management.fee.is_zero();

                if performance_fee_changed
                    || management_fee_changed
                    || max_rate_changed
                    || (has_active_fee
                        && (performance_recipient_changed || management_recipient_changed))
                {
                    self.internal_accrue_fee();
                }

                for account in [
                    fees.performance.recipient.clone(),
                    fees.management.recipient.clone(),
                ] {
                    if self.storage_balance_of(account.clone()).is_none() {
                        self.storage_deposit(Some(account), Some(true));
                    }
                }

                if performance_fee_changed {
                    self.fees.performance.fee = performance_fee;
                    Event::PerformanceFeeSet {
                        fee: fees.performance.fee,
                    }
                    .emit();
                }

                if management_fee_changed {
                    self.fees.management.fee = management_fee;
                    Event::ManagementFeeSet {
                        fee: fees.management.fee,
                    }
                    .emit();
                }

                if max_rate_changed {
                    self.fees.max_total_assets_growth_rate = max_total_assets_growth_rate;
                    Event::MaxTotalAssetsGrowthRateSet {
                        max_rate: fees.max_total_assets_growth_rate,
                    }
                    .emit();
                }

                if performance_recipient_changed {
                    self.fees.performance.recipient = fees.performance.recipient.clone();
                    Event::FeeRecipientSet {
                        account: self.fees.performance.recipient.clone(),
                    }
                    .emit();
                }

                if management_recipient_changed {
                    self.fees.management.recipient = fees.management.recipient.clone();
                    Event::ManagementFeeRecipientSet {
                        account: self.fees.management.recipient.clone(),
                    }
                    .emit();
                }

                if !performance_recipient_changed {
                    self.fees.performance.recipient = fees.performance.recipient.clone();
                }

                if !management_recipient_changed {
                    self.fees.management.recipient = fees.management.recipient.clone();
                }
            }
            TimelockedAction::RestrictionsChange { restrictions } => {
                // Tightening restrictions should invalidate any pending relax/unpause.
                if self
                    .revoke_timelocks(|a| matches!(a, TimelockedAction::RestrictionsChange { .. }))
                {
                    Event::RestrictionsChangeRevoked.emit();
                }

                require!(
                    restrictions != &self.gate.restrictions,
                    "No restriction changes"
                );

                self.gate.restrictions = restrictions.clone();
                Event::RestrictionsSet {
                    restrictions: restrictions.clone(),
                }
                .emit();
                self.apply_kernel_pause(matches!(restrictions, Some(Restrictions::Paused)));
            }
            TimelockedAction::CapChange { market, new_cap } => {
                let market_id = self.ensure_market_record(market);

                let mkt = self
                    .market_record_by_id_mut(market_id)
                    .unwrap_or_else(|| panic_with_message("Config not found"));

                let was_enabled = mkt.cfg.enabled;

                if new_cap.0 > 0 {
                    if !was_enabled {
                        mkt.cfg.enabled = true;
                        Event::MarketEnabled { market: market_id }.emit();
                    }
                    mkt.cfg.removable_at = 0;
                }

                Event::SupplyCapSet {
                    market: market_id,
                    new_cap: *new_cap,
                }
                .emit();
                mkt.cfg.cap = *new_cap;
            }
            TimelockedAction::CapGroupChange { cap_group, new_cap } => {
                let record = self
                    .cap_groups
                    .entry(cap_group.clone())
                    .or_insert_with(CapGroupRecord::default);
                record.cap = *new_cap;
                Event::CapGroupSet {
                    cap_group: cap_group.clone(),
                    new_cap: *new_cap,
                }
                .emit();
            }
            TimelockedAction::CapGroupRelativeCapChange {
                cap_group,
                new_relative_cap,
            } => {
                let new_wad = Wad::from(new_relative_cap.0);
                require!(new_wad <= Wad::one(), "relative cap too high");

                let record = self
                    .cap_groups
                    .entry(cap_group.clone())
                    .or_insert_with(CapGroupRecord::default);
                record.relative_cap = new_wad;

                Event::CapGroupRelativeCapSet {
                    cap_group: cap_group.clone(),
                    new_relative_cap: *new_relative_cap,
                }
                .emit();
            }
            TimelockedAction::CapGroupMembership { market, cap_group } => {
                let market_id = *market;

                let (old_group, principal) = {
                    let rec = self.market_record_by_id_or_panic(market_id);

                    if rec.cfg.cap_group_id == *cap_group {
                        return;
                    }

                    (rec.cfg.cap_group_id.clone(), rec.principal)
                };

                if let Some(old_group) = old_group {
                    self.update_cap_group_principal(&old_group, principal, 0);
                }

                if let Some(new_group) = cap_group.clone() {
                    self.update_cap_group_principal(&new_group, 0, principal);
                }

                let rec = self.market_record_by_id_mut_or_panic(market_id);
                rec.cfg.cap_group_id = cap_group.clone();
                Event::CapGroupMembershipSet {
                    market: market_id,
                    cap_group: cap_group.clone(),
                }
                .emit();
            }
            TimelockedAction::MarketRemoval { market } => {
                let market_id = self.market_id_of_or_panic(market);

                let rec = self.market_record_by_id_mut_or_panic(market_id);

                rec.cfg.removable_at = env::block_timestamp();
                Event::MarketRemovalSubmitted {
                    market: market_id,
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
            TimelockedAction::FeesChange { .. } | TimelockedAction::RestrictionsChange { .. } => {
                self.governance_timelocks.timelock_config_ns
            }
            TimelockedAction::CapChange { .. }
            | TimelockedAction::CapGroupChange { .. }
            | TimelockedAction::CapGroupRelativeCapChange { .. }
            | TimelockedAction::CapGroupMembership { .. } => self.governance_timelocks.cap_ns,
            TimelockedAction::MarketRemoval { .. } => self.governance_timelocks.market_removal_ns,
        };

        require!(
            (MIN_TIMELOCK_NS..=MAX_TIMELOCK_NS).contains(&cur),
            "Timelock duration out of bounds"
        );

        require!(
            !shared_gov::queue_has_pending(&self.governance_timelocks.pending_actions, |a| {
                a == action
            }),
            "Change already pending for this action and arguments"
        );

        let now_ns = env::block_timestamp();
        let valid_at_ns = now_ns.saturating_add(cur);
        shared_gov::queue_schedule(
            &mut self.governance_timelocks.pending_actions,
            action.clone(),
            now_ns,
            cur,
        );

        if let TimelockedAction::CapGroupChange { cap_group, new_cap } = action {
            Event::CapGroupRaiseSubmitted {
                cap_group: cap_group.clone(),
                new_cap: *new_cap,
                valid_at_ns: valid_at_ns.into(),
            }
            .emit();
        }

        if let TimelockedAction::CapGroupRelativeCapChange {
            cap_group,
            new_relative_cap,
        } = action
        {
            Event::CapGroupRelativeCapRaiseSubmitted {
                cap_group: cap_group.clone(),
                new_relative_cap: *new_relative_cap,
                valid_at_ns: valid_at_ns.into(),
            }
            .emit();
        }

        if let TimelockedAction::FeesChange { fees } = action {
            Event::FeesChangeSubmitted {
                fees: fees.clone(),
                valid_at_ns: valid_at_ns.into(),
            }
            .emit();
        }

        if let TimelockedAction::RestrictionsChange { restrictions } = action {
            Event::RestrictionsChangeSubmitted {
                restrictions: restrictions.clone(),
                valid_at_ns: valid_at_ns.into(),
            }
            .emit();
        }

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
        shared_gov::queue_take_mature(
            &mut self.governance_timelocks.pending_actions,
            env::block_timestamp(),
            find_fn,
        )
        .unwrap_or_else(|_| panic_with_message("Timelock not elapsed yet"))
    }

    /// Remove all pending actions that match `pred`.
    /// Returns `true` if at least one action was removed.
    fn revoke_timelocks(&mut self, pred: impl Fn(&TimelockedAction) -> bool) -> bool {
        shared_gov::queue_revoke_pending(&mut self.governance_timelocks.pending_actions, pred)
    }
}
