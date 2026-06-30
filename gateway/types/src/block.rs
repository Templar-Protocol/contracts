use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CryptoHash, NearToken};

/// Header summary for a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BlockSummary {
    pub height: u64,
    /// Block timestamp in nanoseconds since the Unix epoch.
    pub timestamp_ns: u64,
    /// Gas price (yoctoNEAR per unit of gas) at this block.
    pub gas_price: NearToken,
    pub hash: CryptoHash,
}
