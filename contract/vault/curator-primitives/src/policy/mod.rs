//! Curator policy types and functions.
//!
//! This module provides the core policy primitives used by curators to manage
//! vault allocations across markets:
//!
//! - [`cap_group`]: Maximum allocation caps per market group
//! - [`cooldown`]: Reusable cooldown/rate-limiting type
//! - [`supply_queue`]: Pending supply requests
//! - [`withdraw_route`]: How to withdraw from markets
//! - [`refresh_plan`]: List of targets to refresh
//! - [`market_lock`]: Prevent concurrent operations on the same market
//! - [`state`]: Aggregate policy state for executors

pub mod cap_group;
pub mod cap_group_adapter;
pub mod cooldown;
pub mod lock_filter;
pub mod market_lock;
pub mod refresh_plan;
pub mod state;
pub mod supply_queue;
pub mod target_set;
pub mod withdraw_route;
