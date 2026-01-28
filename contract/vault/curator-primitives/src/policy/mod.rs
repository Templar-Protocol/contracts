//! Curator policy types and functions.
//!
//! This module provides the core policy primitives used by curators to manage
//! vault allocations across markets:
//!
//! - [`cap_group`]: Maximum allocation caps per market group
//! - [`supply_queue`]: Pending supply requests
//! - [`withdraw_route`]: How to withdraw from markets
//! - [`refresh_plan`]: List of targets to refresh
//! - [`market_lock`]: Prevent concurrent operations on the same market

pub mod cap_group;
pub mod market_lock;
pub mod refresh_plan;
pub mod supply_queue;
pub mod withdraw_route;
