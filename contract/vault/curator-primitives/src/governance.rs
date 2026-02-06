//! Chain-agnostic governance helpers for vault executors.
//!
//! These helpers encapsulate the portable parts of governance logic:
//! timelock queue mechanics, fee/cap change validation, and restriction
//! relaxation checks. Chain-specific authorization and storage live in
//! each executor, but the decision math is shared here.

use alloc::collections::{BTreeSet, VecDeque};
use alloc::vec::Vec;

use templar_vault_kernel::math::wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD};
use templar_vault_kernel::types::TimestampNs;

/// A pending governance value gated by a timelock.
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize, borsh::BorshSchema))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingValue<T> {
    pub value: T,
    pub valid_at_ns: TimestampNs,
}

impl<T> PendingValue<T> {
    /// Create a new pending value.
    #[must_use]
    pub fn new(value: T, valid_at_ns: TimestampNs) -> Self {
        Self { value, valid_at_ns }
    }

    /// Returns true if the timelock has elapsed.
    #[must_use]
    pub fn is_mature(&self, now_ns: TimestampNs) -> bool {
        now_ns >= self.valid_at_ns
    }
}

/// Timelock queue helpers.
pub fn queue_len<T>(queue: &VecDeque<PendingValue<T>>) -> usize {
    queue.len()
}

pub fn queue_has_pending<T>(queue: &VecDeque<PendingValue<T>>) -> bool {
    !queue.is_empty()
}

pub fn queue_pending_values<T: Clone>(queue: &VecDeque<PendingValue<T>>) -> Vec<PendingValue<T>> {
    queue.iter().cloned().collect()
}

pub fn queue_seek<T>(
    queue: &VecDeque<PendingValue<T>>,
    find_fn: impl Fn(&T) -> bool,
) -> Option<(usize, &PendingValue<T>)> {
    queue
        .iter()
        .enumerate()
        .find(|(_, entry)| find_fn(&entry.value))
}

pub fn queue_remove<T>(
    queue: &mut VecDeque<PendingValue<T>>,
    find_fn: impl Fn(&T) -> bool,
) -> Option<PendingValue<T>> {
    let (idx, _) = queue_seek(queue, find_fn)?;
    queue.remove(idx)
}

pub fn queue_schedule<T>(
    queue: &mut VecDeque<PendingValue<T>>,
    value: T,
    now_ns: TimestampNs,
    timelock_ns: TimestampNs,
) {
    let valid_at_ns = now_ns.saturating_add(timelock_ns);
    queue.push_back(PendingValue::new(value, valid_at_ns));
}

/// Decision on whether an action should be timelocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelockDecision {
    Immediate,
    Timelocked,
}

impl TimelockDecision {
    #[must_use]
    pub fn requires_timelock(self) -> bool {
        matches!(self, TimelockDecision::Timelocked)
    }
}

/// Generic restrictions enum for shared governance checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restrictions<T> {
    Paused,
    BlackList(BTreeSet<T>),
    WhiteList(BTreeSet<T>),
}

/// Determine if a restriction change is relaxing (thus usually timelocked).
#[must_use]
pub fn determine_relaxed<T: Ord>(
    current: &Option<Restrictions<T>>,
    next: &Option<Restrictions<T>>,
) -> bool {
    match (current, next) {
        (None, None) => false,
        (None, Some(_)) => false,
        (Some(_), None) => true,
        (Some(Restrictions::Paused), Some(Restrictions::Paused)) => false,
        (Some(Restrictions::Paused), Some(_)) => true,
        (Some(Restrictions::BlackList(old)), Some(Restrictions::BlackList(new))) => {
            old.difference(new).next().is_some()
        }
        (Some(Restrictions::WhiteList(old)), Some(Restrictions::WhiteList(new))) => {
            new.difference(old).next().is_some()
        }
        (Some(Restrictions::BlackList(old)), Some(Restrictions::WhiteList(new))) => {
            old.intersection(new).next().is_some()
        }
        (Some(Restrictions::WhiteList(_)), Some(Restrictions::Paused))
        | (Some(Restrictions::BlackList(_)), Some(Restrictions::Paused)) => false,
        (Some(Restrictions::WhiteList(_)), Some(Restrictions::BlackList(_))) => true,
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

impl<'a, R> FeeConfig<'a, R> {
    #[must_use]
    pub fn new(
        performance_fee: Wad,
        management_fee: Wad,
        performance_recipient: &'a R,
        management_recipient: &'a R,
        max_rate: Option<Wad>,
    ) -> Self {
        Self {
            performance_fee,
            management_fee,
            performance_recipient,
            management_recipient,
            max_rate,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeChangeDecision {
    pub timelocked: bool,
    pub fee_increase: bool,
    pub recipient_changed: bool,
    pub max_rate_relaxed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeChangeError {
    NoChange,
    PerformanceFeeTooHigh,
    ManagementFeeTooHigh,
}

pub fn evaluate_fee_change<R: PartialEq>(
    current: &FeeConfig<R>,
    proposed: &FeeConfig<R>,
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
    let management_recipient_changed = proposed.management_recipient != current.management_recipient;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelockConfigError {
    NoChange,
    OutOfBounds,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapChangeError {
    NoChange,
}

pub fn cap_change_decision(
    current: Option<u128>,
    proposed: u128,
) -> Result<TimelockDecision, CapChangeError> {
    match current {
        Some(existing) => match proposed.cmp(&existing) {
            core::cmp::Ordering::Equal => Err(CapChangeError::NoChange),
            core::cmp::Ordering::Greater => Ok(TimelockDecision::Timelocked),
            core::cmp::Ordering::Less => Ok(TimelockDecision::Immediate),
        },
        None => Ok(TimelockDecision::Timelocked),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativeCapChangeError {
    NoChange,
    RelativeCapTooHigh,
}

pub fn relative_cap_change_decision(
    current: Option<Wad>,
    proposed: Wad,
) -> Result<TimelockDecision, RelativeCapChangeError> {
    if proposed > Wad::one() {
        return Err(RelativeCapChangeError::RelativeCapTooHigh);
    }

    match current {
        Some(existing) => match proposed.cmp(&existing) {
            core::cmp::Ordering::Equal => Err(RelativeCapChangeError::NoChange),
            core::cmp::Ordering::Greater => Ok(TimelockDecision::Timelocked),
            core::cmp::Ordering::Less => Ok(TimelockDecision::Immediate),
        },
        None => Ok(TimelockDecision::Timelocked),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipChangeError {
    NoChange,
}

pub fn membership_change_decision(changed: bool) -> Result<TimelockDecision, MembershipChangeError> {
    if changed {
        Ok(TimelockDecision::Timelocked)
    } else {
        Err(MembershipChangeError::NoChange)
    }
}

#[must_use]
pub fn market_removal_decision(principal: u128) -> TimelockDecision {
    if principal > 0 {
        TimelockDecision::Timelocked
    } else {
        TimelockDecision::Immediate
    }
}

#[must_use]
pub fn guardian_change_decision(has_guardian: bool) -> TimelockDecision {
    if has_guardian {
        TimelockDecision::Timelocked
    } else {
        TimelockDecision::Immediate
    }
}

#[must_use]
pub fn sentinel_change_decision(has_sentinel: bool) -> TimelockDecision {
    if has_sentinel {
        TimelockDecision::Timelocked
    } else {
        TimelockDecision::Immediate
    }
}
