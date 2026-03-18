//! Chain-agnostic governance helpers for vault executors.
//!
//! These helpers encapsulate the portable parts of governance logic:
//! timelock queue mechanics, fee/cap change validation, and restriction
//! relaxation checks. Chain-specific authorization and storage live in
//! each executor, but the decision math is shared here.

use alloc::collections::{BTreeSet, VecDeque};
use core::cmp::Ordering;

use templar_vault_kernel::math::wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD};
use templar_vault_kernel::types::TimestampNs;
use templar_vault_kernel::TimeGate;

/// A pending governance value gated by a timelock.
#[templar_vault_macros::vault_derive(borsh, schemars, serde, std_borsh_schema)]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingValue<T> {
    pub value: T,
    pub valid_at_ns: TimestampNs,
}

impl<T> PendingValue<T> {
    /// Returns true if the timelock has elapsed.
    #[must_use]
    pub fn is_mature(&self, now_ns: TimestampNs) -> bool {
        TimeGate::from_ready_at(self.valid_at_ns).is_ready(now_ns)
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PendingQueueError {
    NotMature,
}

/// Timelocked pending governance values.
#[templar_vault_macros::vault_derive(borsh, schemars, serde, std_borsh_schema)]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingQueue<T> {
    entries: VecDeque<PendingValue<T>>,
}

impl<T> Default for PendingQueue<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<T> PendingQueue<T> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &PendingValue<T>> {
        self.entries.iter()
    }

    #[must_use]
    pub fn back(&self) -> Option<&PendingValue<T>> {
        self.entries.back()
    }

    pub fn push_pending(&mut self, pending: PendingValue<T>) {
        self.entries.push_back(pending);
    }

    /// Schedule a new timelocked value.
    pub fn schedule(&mut self, value: T, now_ns: TimestampNs, timelock_ns: TimestampNs) {
        let valid_at_ns = TimeGate::schedule_from(now_ns, timelock_ns)
            .ready_at_ns()
            .unwrap_or(now_ns);
        self.entries.push_back(PendingValue { value, valid_at_ns });
    }

    #[must_use]
    pub fn has_pending(&self, mut pred: impl FnMut(&T) -> bool) -> bool {
        self.entries.iter().any(|entry| pred(&entry.value))
    }

    pub fn take_mature(
        &mut self,
        now_ns: TimestampNs,
        mut pred: impl FnMut(&T) -> bool,
    ) -> Result<Option<T>, PendingQueueError> {
        // Find the first entry that matches the predicate AND is mature.
        // This prevents a stale locked entry from blocking a mature one behind it.
        let mature_index = self
            .entries
            .iter()
            .position(|entry| pred(&entry.value) && entry.is_mature(now_ns));

        if let Some(index) = mature_index {
            let Some(pending) = self.entries.remove(index) else {
                return Ok(None);
            };
            return Ok(Some(pending.value));
        }

        // No mature match found - check if there's any match at all (immature).
        let has_immature_match = self.entries.iter().any(|entry| pred(&entry.value));
        if has_immature_match {
            return Err(PendingQueueError::NotMature);
        }

        Ok(None)
    }

    #[must_use]
    pub fn revoke_pending(&mut self, mut pred: impl FnMut(&T) -> bool) -> bool {
        let mut removed_any = false;
        self.entries.retain(|entry| {
            let keep = !pred(&entry.value);
            if !keep {
                removed_any = true;
            }
            keep
        });
        removed_any
    }
}

impl<T> From<VecDeque<PendingValue<T>>> for PendingQueue<T> {
    fn from(entries: VecDeque<PendingValue<T>>) -> Self {
        Self { entries }
    }
}

impl<T> From<PendingQueue<T>> for VecDeque<PendingValue<T>> {
    fn from(queue: PendingQueue<T>) -> Self {
        queue.entries
    }
}

#[must_use]
pub fn submission_requires_timelock<E>(decision: Result<TimelockDecision, E>) -> Result<bool, E> {
    decision.map(TimelockDecision::requires_timelock)
}

/// Decision on whether an action should be timelocked.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimelockDecision {
    Immediate,
    Timelocked,
}

impl TimelockDecision {
    #[must_use]
    pub fn requires_timelock(self) -> bool {
        matches!(self, TimelockDecision::Timelocked)
    }

    #[must_use]
    pub fn from_requires_timelock(requires_timelock: bool) -> Self {
        if requires_timelock {
            TimelockDecision::Timelocked
        } else {
            TimelockDecision::Immediate
        }
    }

    #[must_use]
    pub fn is_immediate(self) -> bool {
        matches!(self, TimelockDecision::Immediate)
    }
}

impl TryFrom<Ordering> for TimelockDecision {
    type Error = ();

    fn try_from(ordering: Ordering) -> Result<Self, Self::Error> {
        match ordering {
            Ordering::Equal => Err(()),
            Ordering::Greater => Ok(TimelockDecision::Timelocked),
            Ordering::Less => Ok(TimelockDecision::Immediate),
        }
    }
}

/// Generic restrictions enum for shared governance checks.
#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum Restrictions<T> {
    Paused,
    Blacklist(BTreeSet<T>),
    Whitelist(BTreeSet<T>),
}

impl<T: Ord> Restrictions<T> {
    /// Determine if a restriction change is relaxing (thus usually timelocked).
    #[must_use]
    pub fn determine_relaxed(current: &Option<Self>, next: &Option<Self>) -> bool {
        match (current, next) {
            (None, None) => false,
            (None, Some(_)) => false,
            (Some(_), None) => true,
            (Some(Self::Paused), Some(Self::Paused)) => false,
            (Some(Self::Paused), Some(Self::Whitelist(new))) => !new.is_empty(),
            (Some(Self::Paused), Some(_)) => true,
            (Some(Self::Blacklist(old)), Some(Self::Blacklist(new))) => {
                old.difference(new).next().is_some()
            }
            (Some(Self::Whitelist(old)), Some(Self::Whitelist(new))) => {
                new.difference(old).next().is_some()
            }
            (Some(Self::Blacklist(old)), Some(Self::Whitelist(new))) => {
                old.intersection(new).next().is_some()
            }
            (Some(Self::Whitelist(_)), Some(Self::Paused))
            | (Some(Self::Blacklist(_)), Some(Self::Paused)) => false,
            (Some(Self::Whitelist(_)), Some(Self::Blacklist(_))) => true,
        }
    }
}

/// Fee config view for change evaluation.
pub struct FeeConfig<'a, R> {
    pub performance_fee: Wad,
    pub management_fee: Wad,
    pub performance_recipient: &'a R,
    pub management_recipient: &'a R,
    pub max_rate: Option<Wad>,
}

impl<R: PartialEq> FeeConfig<'_, R> {
    #[must_use]
    pub fn evaluate_change(
        current: &Self,
        proposed: &Self,
    ) -> Result<FeeChangeDecision, FeeChangeError> {
        if proposed.performance_fee > Wad::from(MAX_PERFORMANCE_FEE_WAD) {
            return Err(FeeChangeError::PerformanceFeeTooHigh);
        }
        if proposed.management_fee > Wad::from(MAX_MANAGEMENT_FEE_WAD) {
            return Err(FeeChangeError::ManagementFeeTooHigh);
        }

        let performance_fee_changed = proposed.performance_fee != current.performance_fee;
        let management_fee_changed = proposed.management_fee != current.management_fee;
        let performance_recipient_changed =
            proposed.performance_recipient != current.performance_recipient;
        let management_recipient_changed =
            proposed.management_recipient != current.management_recipient;
        let max_rate_changed = proposed.max_rate != current.max_rate;

        if !(performance_fee_changed
            || management_fee_changed
            || performance_recipient_changed
            || management_recipient_changed
            || max_rate_changed)
        {
            return Err(FeeChangeError::NoChange);
        }

        let fee_increase = proposed.performance_fee > current.performance_fee
            || proposed.management_fee > current.management_fee;
        let recipient_changed = performance_recipient_changed || management_recipient_changed;

        let max_rate_relaxed = match (current.max_rate, proposed.max_rate) {
            (None, None) => false,
            (None, Some(_)) => false,
            (Some(_), None) => true,
            (Some(old), Some(new)) => new > old,
        };

        Ok(FeeChangeDecision {
            timelocked: fee_increase || recipient_changed || max_rate_relaxed,
            fee_increase,
            recipient_changed,
            max_rate_relaxed,
        })
    }
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FeeChangeDecision {
    pub timelocked: bool,
    pub fee_increase: bool,
    pub recipient_changed: bool,
    pub max_rate_relaxed: bool,
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FeeChangeError {
    NoChange,
    PerformanceFeeTooHigh,
    ManagementFeeTooHigh,
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimelockConfigError {
    NoChange,
    OutOfBounds,
}

#[must_use]
pub fn timelock_config_decision(
    current: TimestampNs,
    proposed: TimestampNs,
    min: TimestampNs,
    max: TimestampNs,
) -> Result<TimelockDecision, TimelockConfigError> {
    if proposed == current {
        return Err(TimelockConfigError::NoChange);
    }
    if proposed < min || proposed > max {
        return Err(TimelockConfigError::OutOfBounds);
    }
    if proposed < current {
        Ok(TimelockDecision::Timelocked)
    } else {
        Ok(TimelockDecision::Immediate)
    }
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CapChangeError {
    NoChange,
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RelativeCapChangeError {
    NoChange,
    RelativeCapTooHigh,
}

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MembershipChangeError {
    NoChange,
}

impl TimelockDecision {
    /// Decide timelock behavior for market caps.
    ///
    /// `None` means the market has no existing cap record yet, so setting a cap is
    /// treated as timelocked.
    #[must_use]
    pub fn from_cap_change(current: Option<u128>, proposed: u128) -> Result<Self, CapChangeError> {
        match current {
            Some(existing) => {
                Self::try_from(proposed.cmp(&existing)).map_err(|_| CapChangeError::NoChange)
            }
            None => Ok(Self::Timelocked),
        }
    }

    /// Decide timelock behavior for optional caps where `0` (or `None`) means unlimited.
    ///
    /// This is intended for cap-group absolute caps, where moving from unlimited to a finite
    /// cap tightens policy and should be immediate, while moving from finite to unlimited
    /// relaxes policy and should be timelocked.
    #[must_use]
    pub fn from_cap_group_cap_change(
        current: Option<u128>,
        proposed: u128,
    ) -> Result<Self, CapChangeError> {
        let normalize = |cap: Option<u128>| cap.and_then(core::num::NonZeroU128::new);
        let current_cap = normalize(current);
        let proposed_cap = core::num::NonZeroU128::new(proposed);

        match (current_cap, proposed_cap) {
            (None, None) => Err(CapChangeError::NoChange),
            (None, Some(_)) => Ok(Self::Immediate),
            (Some(_), None) => Ok(Self::Timelocked),
            (Some(existing), Some(next)) => Self::try_from(next.get().cmp(&existing.get()))
                .map_err(|_| CapChangeError::NoChange),
        }
    }

    #[must_use]
    pub fn from_relative_cap_change(
        current: Option<Wad>,
        proposed: Wad,
    ) -> Result<Self, RelativeCapChangeError> {
        if proposed > Wad::one() {
            return Err(RelativeCapChangeError::RelativeCapTooHigh);
        }

        match current {
            Some(existing) => Self::try_from(proposed.cmp(&existing))
                .map_err(|_| RelativeCapChangeError::NoChange),
            None => Ok(Self::Timelocked),
        }
    }

    #[must_use]
    pub fn from_membership_change(changed: bool) -> Result<Self, MembershipChangeError> {
        if changed {
            Ok(Self::Timelocked)
        } else {
            Err(MembershipChangeError::NoChange)
        }
    }
}
