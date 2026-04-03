//! Chain-agnostic governance helpers for vault executors.
//!
//! These helpers encapsulate the portable parts of governance logic:
//! timelock queue mechanics, fee/cap change validation, and restriction
//! relaxation checks. Chain-specific authorization and storage live in
//! each executor, but the decision math is shared here.

use alloc::{
    collections::{BTreeSet, VecDeque},
    vec::Vec,
};

use templar_vault_kernel::math::wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD};
use templar_vault_kernel::types::{DurationNs, TimestampNs};
use templar_vault_kernel::TimeGate;

/// A pending governance value gated by a timelock.
#[templar_vault_macros::vault_derive(borsh, schemars, serde, std_borsh_schema)]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingValue<T> {
    pub value: T,
    pub ready_at_ns: TimestampNs,
}

impl<T> PendingValue<T> {
    /// Returns true if the timelock has elapsed.
    #[must_use]
    pub fn is_mature(&self, now_ns: TimestampNs) -> bool {
        TimeGate::from_ready_at(self.ready_at_ns).is_ready(now_ns)
    }
}

#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum TakePending<T> {
    Missing,
    Pending { ready_at_ns: TimestampNs },
    Ready(T),
}

pub struct ScheduledPending<T> {
    pub ready_at_ns: TimestampNs,
    pub replaced: Vec<T>,
}

#[templar_vault_macros::vault_derive(borsh, schemars, serde, std_borsh_schema)]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingActions<T> {
    entries: VecDeque<PendingValue<T>>,
}

impl<T> Default for PendingActions<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<T> PendingActions<T> {
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

    pub fn schedule(
        &mut self,
        value: T,
        now_ns: TimestampNs,
        timelock_ns: DurationNs,
    ) -> TimestampNs {
        let ready_at_ns = TimeGate::schedule_from(now_ns, timelock_ns)
            .ready_at_ns()
            .expect("TimeGate::schedule_from always yields a ready timestamp");
        self.entries.push_back(PendingValue { value, ready_at_ns });
        ready_at_ns
    }

    #[must_use]
    pub fn has_pending_key<K: PartialEq>(&self, key: &K, mut key_of: impl FnMut(&T) -> K) -> bool {
        self.entries
            .iter()
            .any(|entry| key_of(&entry.value) == *key)
    }

    pub fn take_by_key<K: PartialEq>(
        &mut self,
        now_ns: TimestampNs,
        key: &K,
        mut key_of: impl FnMut(&T) -> K,
    ) -> TakePending<T> {
        let mut mature_index = None;
        let mut next_ready_at: Option<TimestampNs> = None;

        for (index, entry) in self.entries.iter().enumerate() {
            if key_of(&entry.value) != *key {
                continue;
            }

            if entry.is_mature(now_ns) {
                mature_index = Some(index);
                break;
            }

            next_ready_at = Some(match next_ready_at {
                Some(current) => current.min(entry.ready_at_ns),
                None => entry.ready_at_ns,
            });
        }

        if let Some(index) = mature_index {
            let pending = self
                .entries
                .remove(index)
                .expect("matched pending index must remain valid");
            return TakePending::Ready(pending.value);
        }

        match next_ready_at {
            Some(ready_at_ns) => TakePending::Pending { ready_at_ns },
            None => TakePending::Missing,
        }
    }

    #[must_use]
    pub fn revoke_by_key<K: PartialEq>(
        &mut self,
        key: &K,
        mut key_of: impl FnMut(&T) -> K,
    ) -> Vec<T> {
        let mut retained = VecDeque::with_capacity(self.entries.len());
        let mut removed = Vec::new();

        while let Some(entry) = self.entries.pop_front() {
            if key_of(&entry.value) == *key {
                removed.push(entry.value);
            } else {
                retained.push_back(entry);
            }
        }

        self.entries = retained;
        removed
    }

    pub fn schedule_replacing<K: PartialEq>(
        &mut self,
        key: &K,
        key_of: impl FnMut(&T) -> K,
        value: T,
        now_ns: TimestampNs,
        timelock_ns: DurationNs,
    ) -> ScheduledPending<T> {
        let replaced = self.revoke_by_key(key, key_of);
        let ready_at_ns = self.schedule(value, now_ns, timelock_ns);
        ScheduledPending {
            ready_at_ns,
            replaced,
        }
    }

    #[must_use]
    pub fn from_restored_entries(entries: VecDeque<PendingValue<T>>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn into_entries(self) -> VecDeque<PendingValue<T>> {
        self.entries
    }
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

pub fn timelock_config_decision(
    current: DurationNs,
    proposed: DurationNs,
    min: DurationNs,
    max: DurationNs,
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

#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MembershipChangeKind {
    Added,
    Removed,
    Reassigned,
}

impl TimelockDecision {
    /// Decide timelock behavior for market caps.
    ///
    /// `None` means the market has no existing cap record yet, so setting a cap is
    /// treated as timelocked.
    pub fn from_cap_change(current: Option<u128>, proposed: u128) -> Result<Self, CapChangeError> {
        match current {
            Some(existing) if proposed == existing => Err(CapChangeError::NoChange),
            Some(existing) if proposed > existing => Ok(Self::Timelocked),
            Some(_) => Ok(Self::Immediate),
            None => Ok(Self::Timelocked),
        }
    }

    pub fn from_cap_group_cap_change(
        current: Option<u128>,
        proposed: Option<u128>,
    ) -> Result<Self, CapChangeError> {
        match (current, proposed) {
            (None, None) => Err(CapChangeError::NoChange),
            (None, Some(_)) => Ok(Self::Immediate),
            (Some(_), None) => Ok(Self::Timelocked),
            (Some(existing), Some(next)) if next == existing => Err(CapChangeError::NoChange),
            (Some(existing), Some(next)) if next > existing => Ok(Self::Timelocked),
            (Some(_), Some(_)) => Ok(Self::Immediate),
        }
    }

    pub fn from_relative_cap_change(
        current: Option<Wad>,
        proposed: Option<Wad>,
    ) -> Result<Self, RelativeCapChangeError> {
        if let Some(proposed) = proposed {
            if proposed > Wad::one() {
                return Err(RelativeCapChangeError::RelativeCapTooHigh);
            }
        }

        match (current, proposed) {
            (None, None) => Err(RelativeCapChangeError::NoChange),
            (None, Some(_)) => Ok(Self::Timelocked),
            (Some(_), None) => Ok(Self::Immediate),
            (Some(existing), Some(next)) if next == existing => {
                Err(RelativeCapChangeError::NoChange)
            }
            (Some(existing), Some(next)) if next > existing => Ok(Self::Timelocked),
            (Some(_), Some(_)) => Ok(Self::Immediate),
        }
    }

    #[must_use]
    pub fn membership_change_kind<T: PartialEq>(
        current: Option<&T>,
        proposed: Option<&T>,
    ) -> Option<MembershipChangeKind> {
        match (current, proposed) {
            (None, None) => None,
            (None, Some(_)) => Some(MembershipChangeKind::Added),
            (Some(_), None) => Some(MembershipChangeKind::Removed),
            (Some(current), Some(proposed)) if current == proposed => None,
            (Some(_), Some(_)) => Some(MembershipChangeKind::Reassigned),
        }
    }

    pub fn from_membership_assignment_change<T: PartialEq>(
        current: Option<&T>,
        proposed: Option<&T>,
    ) -> Result<Self, MembershipChangeError> {
        match Self::membership_change_kind(current, proposed) {
            Some(_) => Ok(Self::Timelocked),
            None => Err(MembershipChangeError::NoChange),
        }
    }

    #[must_use]
    pub fn from_membership_change_kind(_change: MembershipChangeKind) -> Self {
        Self::Timelocked
    }
}
