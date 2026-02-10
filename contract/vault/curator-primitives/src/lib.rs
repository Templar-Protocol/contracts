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
pub mod rbac;
pub mod policy;
pub mod recovery;
pub mod governance;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod test_utils;

// Re-exports for convenience
pub use auth::{ActionKind, AuthAdapter, AuthError, AuthResult, PermissiveAuth, StrictAuth};
pub use rbac::{RbacAuth, RbacConfig, Role, RoleAssignment};

pub use policy::{
    cap_group::{CapGroup, CapGroupError, CapGroupId, CapGroupRecord},
    cooldown::{Cooldown, CooldownError},
    market_lock::{MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan, RefreshPlanError},
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

pub use recovery::{
    determine_recovery_action, handle_allocation_failure, handle_payout_failure,
    handle_payout_failure_default, handle_refresh_failure, handle_withdrawal_failure,
    RecoveryContext, RecoveryOutcome, RecoveryProgress,
};

pub use governance::{
    cap_change_decision, determine_relaxed, evaluate_fee_change, guardian_change_decision,
    market_removal_decision, membership_change_decision, queue_schedule,
    relative_cap_change_decision, sentinel_change_decision, timelock_config_decision,
    FeeChangeDecision, FeeChangeError, FeeConfig, MembershipChangeError, PendingValue,
    Restrictions, TimelockConfigError, TimelockDecision,
};
