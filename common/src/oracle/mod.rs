use near_sdk::{near, AccountId};
use pyth::PriceIdentifier;

pub mod price_transformer;
pub mod proxy;
pub mod pyth;
pub mod redstone;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub enum OracleRequest {
    Pyth(PythRequest),
    RedStone(RedStoneRequest),
}

impl OracleRequest {
    pub fn oracle_id(&self) -> &near_sdk::AccountId {
        match self {
            OracleRequest::Pyth(id) => &id.oracle_id,
            OracleRequest::RedStone(id) => &id.oracle_id,
        }
    }

    pub fn pyth(oracle_id: AccountId, price_id: PriceIdentifier) -> Self {
        Self::Pyth(PythRequest {
            oracle_id,
            price_id,
        })
    }

    pub fn redstone(
        oracle_id: AccountId,
        price_id: impl Into<crate::oracle::redstone::FeedId>,
    ) -> Self {
        Self::RedStone(RedStoneRequest {
            oracle_id,
            price_id: price_id.into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct PythRequest {
    pub oracle_id: near_sdk::AccountId,
    pub price_id: PriceIdentifier,
}

impl From<PythRequest> for OracleRequest {
    fn from(id: PythRequest) -> Self {
        Self::Pyth(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct RedStoneRequest {
    pub oracle_id: near_sdk::AccountId,
    pub price_id: crate::oracle::redstone::FeedId,
}

impl From<RedStoneRequest> for OracleRequest {
    fn from(id: RedStoneRequest) -> Self {
        Self::RedStone(id)
    }
}
