use near_sdk::{json_types::U64, near};

use crate::oracle::pyth::PriceIdentifier;

use super::Proxy;

#[near(event_json(standard = "templar-proxy-oracle-governance"))]
pub enum ProxyOracleEvent {
    /// When a new proposal is created.
    #[event_version("1.0.0")]
    Proposal { op_id: u32, proposal: Proposal },
    /// When a proposal is cancelled.
    #[event_version("1.0.0")]
    Cancellation { op_id: u32, proposal: Proposal },
    /// When a proposal is executed.
    #[event_version("1.0.0")]
    Execution { op_id: u32, operation: Operation },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Proposal {
    pub operation: Operation,
    pub created_at_ms: U64,
}

impl Proposal {
    pub fn can_execute(&self, now_ms: u64, ttl_ms: u64) -> bool {
        now_ms.saturating_sub(self.created_at_ms.0) >= ttl_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Operation {
    SetProxy {
        id: PriceIdentifier,
        proxy: Option<Proxy>,
    },
    SetActionTtl {
        new_ttl_ms: U64,
    },
}
