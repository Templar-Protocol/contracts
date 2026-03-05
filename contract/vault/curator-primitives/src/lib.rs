//! Chain-agnostic curator primitives for Templar Protocol vaults.
//!
//! This crate provides shared curator policy and recovery logic that can be used
//! by both NEAR and Soroban vault executors. It depends only on `templar-vault-kernel`
//! types and contains no chain-specific SDK dependencies.
//!
//! # Modules
//!
//! - [`auth`]: Pluggable authentication and authorization primitives
//! - [`rbac`]: Role-based access control adapter
//! - [`policy`]: Cap groups, supply queues, withdraw routes, refresh plans, and market locks
//! - [`recovery`]: Recovery action determination and state machine recovery logic
//!
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod auth;
pub mod governance;
pub mod policy;
pub mod rbac;
#[cfg(feature = "recovery")]
pub mod recovery;
pub mod utils;

pub use auth::{
    boundary_policy_class, canonical_policy_class, ActionKind, AuthAdapter, AuthError,
    AuthPolicyClass, AuthResult,
};
pub use rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};

pub use policy::{
    cap_group::{
        CapGroup, CapGroupError, CapGroupId, CapGroupRecord, CapGroupUpdate, CapGroupUpdateKey,
    },
    cap_group_adapter::{
        available_capacity_from_fields, can_allocate_from_fields, cap_group_from_fields,
        cap_group_record_absolute_cap, cap_group_record_from_fields, cap_group_record_relative_cap,
        effective_cap_from_fields, enforce_from_fields, set_cap_group_record_absolute_cap,
        set_cap_group_record_relative_cap,
    },
    cooldown::{Cooldown, CooldownError},
    market_lock::{validate_lock_expiry, MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan, RefreshPlanError},
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    target_set::{
        build_refresh_plan_from_targets, build_withdraw_plan_from_target_principals,
        find_duplicate_target_id, find_first_duplicate, find_locked_targets, get_locked_targets,
        has_unique_items, is_target_locked, validate_no_duplicate_targets,
    },
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

#[cfg(feature = "recovery")]
pub use recovery::{
    determine_recovery_action, handle_allocation_failure, handle_payout_failure,
    handle_payout_failure_default, handle_refresh_failure, handle_withdrawal_failure,
    RecoveryContext, RecoveryOutcome, RecoveryProgress,
};

pub use governance::{
    timelock_config_decision, FeeChangeDecision, FeeChangeError, FeeConfig, MembershipChangeError,
    PendingQueue, PendingQueueError, PendingValue, Restrictions, TimelockConfigError,
    TimelockDecision,
};
pub use utils::{nonnegative_i128_to_u128, seconds_to_nanoseconds, u128_to_i128_checked};

#[cfg(test)]
mod tests;
