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
    let valid_at_ns = TimeGate::schedule_from(now_ns, timelock_ns)
        .ready_at_ns()
        .unwrap_or(now_ns);
    queue.push_back(PendingValue::new(value, valid_at_ns));
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PendingQueueError {
    NotMature,
}

#[must_use]
pub fn queue_has_pending<T>(
    queue: &VecDeque<PendingValue<T>>,
    mut pred: impl FnMut(&T) -> bool,
) -> bool {
    queue.iter().any(|entry| pred(&entry.value))
}

pub fn queue_take_mature<T>(
    queue: &mut VecDeque<PendingValue<T>>,
    now_ns: TimestampNs,
    mut pred: impl FnMut(&T) -> bool,
) -> Result<Option<T>, PendingQueueError> {
    let Some(index) = queue.iter().position(|entry| pred(&entry.value)) else {
        return Ok(None);
    };

    let Some(entry) = queue.get(index) else {
        return Ok(None);
    };

    if !entry.is_mature(now_ns) {
        return Err(PendingQueueError::NotMature);
    }

    let Some(pending) = queue.remove(index) else {
        return Ok(None);
    };
    Ok(Some(pending.value))
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

#[must_use]
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

#[must_use]
fn timelock_decision_from_cmp(ordering: Ordering) -> Option<TimelockDecision> {
    match ordering {
        Ordering::Equal => None,
        Ordering::Greater => Some(TimelockDecision::Timelocked),
        Ordering::Less => Some(TimelockDecision::Immediate),
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

#[must_use]
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

/// Decide timelock behavior for market caps.
///
/// `None` means the market has no existing cap record yet, so setting a cap is
/// treated as timelocked.
#[must_use]
pub fn cap_change_decision(
    current: Option<u128>,
    proposed: u128,
) -> Result<TimelockDecision, CapChangeError> {
    match current {
        Some(existing) => {
            timelock_decision_from_cmp(proposed.cmp(&existing)).ok_or(CapChangeError::NoChange)
        }
        None => Ok(TimelockDecision::Timelocked),
    }
}

/// Decide timelock behavior for optional caps where `0` (or `None`) means unlimited.
///
/// This is intended for cap-group absolute caps, where moving from unlimited to a finite
/// cap tightens policy and should be immediate, while moving from finite to unlimited
/// relaxes policy and should be timelocked.
#[must_use]
pub fn cap_group_cap_change_decision(
    current: Option<u128>,
    proposed: u128,
) -> Result<TimelockDecision, CapChangeError> {
    let normalize = |cap: Option<u128>| cap.and_then(core::num::NonZeroU128::new);
    let current_cap = normalize(current);
    let proposed_cap = core::num::NonZeroU128::new(proposed);

    match (current_cap, proposed_cap) {
        (None, None) => Err(CapChangeError::NoChange),
        (None, Some(_)) => Ok(TimelockDecision::Immediate),
        (Some(_), None) => Ok(TimelockDecision::Timelocked),
        (Some(existing), Some(next)) => timelock_decision_from_cmp(next.get().cmp(&existing.get()))
            .ok_or(CapChangeError::NoChange),
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

#[must_use]
pub fn relative_cap_change_decision(
    current: Option<Wad>,
    proposed: Wad,
) -> Result<TimelockDecision, RelativeCapChangeError> {
    if proposed > Wad::one() {
        return Err(RelativeCapChangeError::RelativeCapTooHigh);
    }

    match current {
        Some(existing) => timelock_decision_from_cmp(proposed.cmp(&existing))
            .ok_or(RelativeCapChangeError::NoChange),
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

#[must_use]
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
    TimelockDecision::from_requires_timelock(principal > 0)
}

#[must_use]
pub fn guardian_change_decision(has_guardian: bool) -> TimelockDecision {
    TimelockDecision::from_requires_timelock(has_guardian)
}

#[must_use]
pub fn sentinel_change_decision(has_sentinel: bool) -> TimelockDecision {
    TimelockDecision::from_requires_timelock(has_sentinel)
}

#[cfg(test)]
mod tests;
