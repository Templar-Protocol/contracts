//! NEAR protocol limits the gateway enforces before submitting.
//!
//! Chain state, not library constants: `max_total_prepaid_gas` went from 300 to
//! 1000 Tgas, and a pinned `near-parameters` lags mainnet by several protocol
//! versions. Read current values from `EXPERIMENTAL_protocol_config`.

use crate::NearGas;

/// Actions one receipt may carry.
pub const MAX_ACTIONS_PER_RECEIPT: usize = 100;

/// Gas one transaction may prepay across all its actions. Mainnet protocol 86.
pub const MAX_TOTAL_PREPAID_GAS: NearGas = NearGas::from_tgas(1000);

/// Bytes a signed transaction may serialize to. Well below `max_contract_size`,
/// so two deploys can each be valid and not fit together.
pub const MAX_TRANSACTION_SIZE: usize = 1_572_864;
