//! Chain-agnostic governance helpers for vault executors.
//!
//! These helpers encapsulate the portable parts of governance logic:
//! timelock queue mechanics, fee/cap change validation, and restriction
//! relaxation checks. Chain-specific authorization and storage live in
//! each executor, but the decision math is shared here.

use alloc::collections::{BTreeSet, VecDeque};

use templar_vault_kernel::math::wad::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD};
use templar_vault_kernel::types::TimestampNs;
use templar_vault_kernel::TimeGate;

/// A pending governance value gated by a timelock.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(all(feature = "borsh", feature = "std"), derive(borsh::BorshSchema))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
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
        TimeGate::from_ready_at(self.valid_at_ns).is_ready(now_ns)
    }
}

/// Schedule a new timelocked value on the queue.
pub fn queue_schedule<T>(
    queue: &mut VecDeque<PendingValue<T>>,
    value: T,
    now_ns: TimestampNs,
    timelock_ns: TimestampNs,
) {
    let valid_at_ns = match TimeGate::schedule_from(now_ns, timelock_ns).ready_at_ns() {
        Some(timestamp_ns) => timestamp_ns,
        None => now_ns,
    };
    queue.push_back(PendingValue::new(value, valid_at_ns));
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PendingQueueError {
    NotMature,
}

#[must_use]
pub fn queue_has_pending<T>(queue: &VecDeque<PendingValue<T>>, pred: impl Fn(&T) -> bool) -> bool {
    queue.iter().any(|entry| pred(&entry.value))
}

pub fn queue_take_mature<T>(
    queue: &mut VecDeque<PendingValue<T>>,
    now_ns: TimestampNs,
    pred: impl Fn(&T) -> bool,
) -> Result<Option<T>, PendingQueueError> {
    let Some((index, entry)) = queue
        .iter()
        .enumerate()
        .find(|(_, entry)| pred(&entry.value))
    else {
        return Ok(None);
    };

    if !entry.is_mature(now_ns) {
        return Err(PendingQueueError::NotMature);
    }

    let value = queue.remove(index).unwrap_or_else(|| panic!()).value;
    Ok(Some(value))
}

#[must_use]
pub fn queue_revoke_pending<T>(
    queue: &mut VecDeque<PendingValue<T>>,
    pred: impl Fn(&T) -> bool,
) -> bool {
    let mut removed_any = false;
    queue.retain(|entry| {
        let keep = !pred(&entry.value);
        if !keep {
            removed_any = true;
        }
        keep
    });
    removed_any
}

pub fn submission_requires_timelock<E>(decision: Result<TimelockDecision, E>) -> Result<bool, E> {
    decision.map(TimelockDecision::requires_timelock)
}

/// Decision on whether an action should be timelocked.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum Restrictions<T> {
    Paused,
    Blacklist(BTreeSet<T>),
    Whitelist(BTreeSet<T>),
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
        (Some(Restrictions::Blacklist(old)), Some(Restrictions::Blacklist(new))) => {
            old.difference(new).next().is_some()
        }
        (Some(Restrictions::Whitelist(old)), Some(Restrictions::Whitelist(new))) => {
            new.difference(old).next().is_some()
        }
        (Some(Restrictions::Blacklist(old)), Some(Restrictions::Whitelist(new))) => {
            old.intersection(new).next().is_some()
        }
        (Some(Restrictions::Whitelist(_)), Some(Restrictions::Paused))
        | (Some(Restrictions::Blacklist(_)), Some(Restrictions::Paused)) => false,
        (Some(Restrictions::Whitelist(_)), Some(Restrictions::Blacklist(_))) => true,
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

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FeeChangeDecision {
    pub timelocked: bool,
    pub fee_increase: bool,
    pub recipient_changed: bool,
    pub max_rate_relaxed: bool,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum MembershipChangeError {
    NoChange,
}

pub fn membership_change_decision(
    changed: bool,
) -> Result<TimelockDecision, MembershipChangeError> {
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

#[cfg(test)]
mod tests;
