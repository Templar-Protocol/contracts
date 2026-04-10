use near_sdk::near;
use templar_common::{oracle::pyth::PriceIdentifier, time::Nanoseconds};

use super::Proxy;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Operation {
    SetProxy {
        id: PriceIdentifier,
        proxy: Option<Proxy>,
    },
    SetActionTtl {
        new_ttl: Nanoseconds,
    },
}
