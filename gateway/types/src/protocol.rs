//! NEAR protocol limits the gateway enforces before submitting.
//!
//! Chain state, not library constants: a pinned `near-parameters` lags mainnet
//! by several protocol versions. Read current values from
//! `EXPERIMENTAL_protocol_config`.

/// Actions one receipt may carry.
pub const MAX_ACTIONS_PER_RECEIPT: usize = 100;
