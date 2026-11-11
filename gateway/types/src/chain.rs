use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::CryptoHash;

/// Chain identity reported by the node, independent of retained block history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChainIdentity {
    pub chain_id: String,
    pub genesis_hash: CryptoHash,
}
