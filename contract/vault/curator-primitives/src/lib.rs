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
        cap_group_record_absolute_cap, cap_group_record_relative_cap,
        set_cap_group_record_absolute_cap, set_cap_group_record_relative_cap,
    },
    cooldown::{Cooldown, CooldownError},
    duplicate::{find_first_duplicate, has_unique_items},
    market_lock::{
        AcquireLeaseError, FencingError, FencingToken, LeaseDurationNs, LeaseOwner, MarketLease,
        MarketLeaseRegistry, ReleaseLeaseError,
    },
    refresh_plan::{
        build_stale_refresh_plan, RefreshPlan, RefreshPlanError, RefreshTargetStatus,
        RefreshThrottle,
    },
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    target_set::{build_refresh_plan_from_targets, build_withdraw_plan_from_target_principals},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

#[cfg(feature = "recovery")]
pub use recovery::{
    compute_payout_failure_outcome, compute_payout_success_outcome, compute_recovery_stats,
    compute_settlement_shares, determine_recovery_action, plan_allocation_recovery,
    plan_payout_recovery, plan_refresh_recovery, plan_withdrawal_recovery, PayoutRecoveryEvidence,
    RecoveryContext, RecoveryError, RecoveryOutcome, RecoveryPolicy, RecoveryProgress,
};

pub use governance::{
    timelock_config_decision, FeeChangeDecision, FeeChangeError, FeeConfig, MembershipChangeError,
    MembershipChangeKind, PendingQueue, PendingValue, Restrictions, ScheduledPending, TakePending,
    TimelockConfigError, TimelockDecision,
};
pub use utils::{nonnegative_i128_to_u128, seconds_to_nanoseconds, u128_to_i128_checked};

#[cfg(test)]
mod tests;
