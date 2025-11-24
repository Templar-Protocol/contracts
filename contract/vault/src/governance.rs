use super::*;

#[derive(Clone)]
#[near(serializers = [json, borsh])]
pub struct Abdicator {
    map: HashMap<String, bool>,
}

impl Abdicator {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn is_abdicated(&self, method_name: &str) -> bool {
        *self.map.get(&method_name.to_string()).unwrap_or(&false)
    }

    pub fn abdicate(&mut self, method_name: &str) {
        self.map.insert(method_name.to_string(), true);
        near_sdk::env::log_str(&format!("abdicated {method_name}"));
    }

    pub fn require_not_abdicated(method_name: &str) {
        let this = Self::new();
        if this.is_abdicated(method_name) {
            templar_common::panic_with_message(&format!("abdicated {method_name}"));
        }
    }
}

#[near]
impl Contract {
    /// Sets the Curator account. Also grants/removes the Allocator role accordingly.
    pub fn set_curator(&mut self, account: AccountId) {
        Self::require_owner();
        Abdicator::require_not_abdicated("set_curator");
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
        Abdicator::require_not_abdicated("set_is_allocator");
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
        Abdicator::require_not_abdicated("submit_guardian");
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
        Abdicator::require_not_abdicated("accept_guardian");

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
        Abdicator::require_not_abdicated("revoke_pending_guardian");
        self.pending_guardian = None;
    }

    /// Sets the recipient account for skimmed tokens.
    pub fn set_skim_recipient(&mut self, account: AccountId) {
        Self::require_owner();
        Abdicator::require_not_abdicated("set_skim_recipient");
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
        Abdicator::require_not_abdicated("set_fee_recipient");
        require!(account != self.fee_recipient, "Already set to this address");

        if self.performance_fee != wad::Wad::zero() {
            // Accrue any pending fees to current recipient before changing (so current recipient gets up to now)
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

    /// Sets the performance fee as a WAD fraction (1e24 = 100%). Accrues fees at the old rate first.
    pub fn set_performance_fee(&mut self, fee: Wad) {
        Self::require_owner();
        Abdicator::require_not_abdicated("set_performance_fee");

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
        Abdicator::require_not_abdicated("submit_timelock");
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
        Abdicator::require_not_abdicated("accept_timelock");
        if let Some(p) = &self.pending_timelock {
            p.verify();

            self.timelock_ns = p.value;
            Event::TimelockSet {
                seconds: p.value.into(),
            }
            .emit();
            self.pending_timelock = None;
        } else {
            templar_common::panic_with_message("No pending timelock change");
        }
    }

    /// Revokes any pending timelock change.
    pub fn revoke_pending_timelock(&mut self) {
        Self::assert_guardian_or_owner();
        Abdicator::require_not_abdicated("revoke_pending_timelock");
        self.pending_timelock = None;
        Event::PendingTimelockRevoked {}.emit();
    }

    /// Submits a change to a market's supply cap.
    /// Decreases apply immediately; increases are subject to the governance timelock.
    ///
    /// # Panics
    /// If the market does not exist.
    #[payable]
    pub fn submit_cap(&mut self, market: AccountId, new_cap: U128) {
        Self::assert_curator_or_owner();
        Abdicator::require_not_abdicated("submit_cap");
        self.ensure_idle();

        let mkt = match self.markets.get_mut(&market) {
            None => {
                self.markets.insert(market.clone(), MarketRecord::default());
                Event::MarketCreated {
                    market: market.clone(),
                }
                .emit();
                self.markets
                    .get_mut(&market)
                    .unwrap_or_else(|| templar_common::panic_with_message("Config not found"))
            }
            Some(m) => m,
        };

        require!(
            &mkt.pending_cap.is_none(),
            "Policy violation: A cap change is already pending for this market"
        );

        require!(
            mkt.cfg.removable_at == 0,
            "Market removal pending, cannot change cap"
        );

        require!(new_cap != mkt.cfg.cap, "New cap is same as current");

        if new_cap < mkt.cfg.cap {
            // If lowering the cap, we can apply the delta immediately
            mkt.cfg.cap = new_cap;
        } else {
            let valid_at_ns = env::block_timestamp() + self.timelock_ns;
            if let Some(rec) = self.markets.get_mut(&market) {
                rec.pending_cap = Some(PendingValue {
                    value: new_cap.0,
                    valid_at_ns,
                });
            }
            Event::SupplyCapRaiseSubmitted {
                market: market.clone(),
                new_cap,
                valid_at_ns,
            }
            .emit();
        }
    }

    /// Accepts a pending cap increase for `market` once the timelock has elapsed.
    /// # Panics
    /// If the market does not exist.
    #[payable]
    pub fn accept_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        Abdicator::require_not_abdicated("accept_cap");
        self.ensure_idle();

        let m = self
            .markets
            .get_mut(&market)
            .unwrap_or_else(|| templar_common::panic_with_message("Config not found"));

        let was_enabled = m.cfg.enabled;

        let pending_value = m.pending_cap.as_ref().map_or_else(
            || templar_common::panic_with_message("No pending cap change for this market"),
            |pending_cap| {
                pending_cap.verify();
                pending_cap.value
            },
        );
        m.cfg.cap = pending_value.into();

        if pending_value > 0 {
            if !m.cfg.enabled {
                m.cfg.enabled = true;
            }
            m.cfg.removable_at = 0;
        }

        if pending_value > 0 && !was_enabled {
            Event::MarketEnabled {
                market: market.clone(),
            }
            .emit();
        }

        Event::SupplyCapSet {
            market: market.clone(),
            new_cap: U128(pending_value),
        }
        .emit();

        self.markets
            .get_mut(&market)
            .unwrap_or_else(|| templar_common::panic_with_message("Config not found"))
            .pending_cap = None;
    }

    /// Revokes any pending cap change for `market`.
    pub fn revoke_pending_cap(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        Abdicator::require_not_abdicated("revoke_pending_cap");
        if let Some(rec) = self.markets.get_mut(&market) {
            if rec.pending_cap.take().is_some() {
                Event::SupplyCapRaiseRevoked {
                    market: market.clone(),
                }
                .emit();
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
        Self::assert_curator_or_owner();
        Abdicator::require_not_abdicated("submit_market_removal");
        let rec = self.markets.get_mut(&market).unwrap_or_else(|| {
            templar_common::panic_with_message(&format!("Unknown market: {market}"))
        });
        require!(
            rec.cfg.removable_at == 0,
            "Removal already pending for this market"
        );
        require!(
            rec.cfg.cap.0 == 0,
            "Cannot remove market with non-zero cap (disable deposits first)"
        );
        require!(rec.cfg.enabled, "Market not enabled or already removed");
        require!(
            rec.pending_cap.is_none(),
            "Cap change pending for this market"
        );
        rec.cfg.removable_at = env::block_timestamp() + self.timelock_ns;
        Event::MarketRemovalSubmitted {
            market: market.clone(),
            removable_at: rec.cfg.removable_at.into(),
        }
        .emit();
    }

    /// Revokes a pending market removal for `market`.
    pub fn revoke_pending_market_removal(&mut self, market: AccountId) {
        Self::assert_curator_or_owner();
        Abdicator::require_not_abdicated("revoke_pending_market_removal");
        if let Some(cfg) = self.markets.get_mut(&market).map(|c| &mut c.cfg) {
            cfg.removable_at = 0;
        }
        Event::MarketRemovalRevoked { market }.emit();
    }

    /// Sets the ordered supply queue.
    /// Rejects duplicates and markets without a positive cap. Requires the vault to be idle.
    #[payable]
    pub fn set_supply_queue(&mut self, markets: Vec<AccountId>) {
        Self::assert_allocator();
        Abdicator::require_not_abdicated("set_supply_queue");
        self.ensure_idle();
        require!(markets.len() <= MAX_QUEUE_LEN, "too long");

        // Invariant: supply_queue has no duplicates
        let mut seen = HashSet::new();
        for m in &markets {
            if !seen.insert(m.clone()) {
                templar_common::panic_with_message(&format!("Duplicate market {m}"));
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
        match self.abdicator {
            Some(ref mut abdicator) => {
                abdicator.abdicate(&method_name);
            }
            None => {
                let mut abdicator = Abdicator::new();
                abdicator.abdicate(&method_name);
                self.abdicator = Some(abdicator);
            }
        }
    }
}
