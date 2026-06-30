use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CryptoHash, NearToken};

/// Header summary for a block. Carries the block's `gas_price`, so callers that
/// only need a current gas estimate can read it from the latest block instead of
/// a separate gas-price query.
///
/// Shared by the `chain.getBlock` spec result and the core chain client's return
/// type, so the two can't drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BlockSummary {
    pub height: u64,
    /// Block timestamp in nanoseconds since the Unix epoch.
    pub timestamp_ns: u64,
    /// Gas price (yoctoNEAR per unit of gas) at this block.
    pub gas_price: NearToken,
    pub hash: CryptoHash,
}
