//! NEAR protocol limits the gateway enforces before submitting.
//!
//! Chain state, not library constants: a pinned `near-parameters` lags mainnet
//! by several protocol versions. Read current values from
//! `EXPERIMENTAL_protocol_config`.

use near_gas::NearGas;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProtocolLimits {
    pub max_transaction_size: u64,
    pub max_total_prepaid_gas: NearGas,
    pub max_length_storage_key: u64,
    pub num_bytes_account: u64,
    pub num_extra_bytes_record: u64,
    pub max_length_storage_value: u64,
}

/// Actions one receipt may carry.
pub const MAX_ACTIONS_PER_RECEIPT: usize = 100;
