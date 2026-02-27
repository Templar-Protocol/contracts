use near_sdk::near;
use pyth::PriceIdentifier;

pub mod price_transformer;
pub mod proxy;
pub mod pyth;
#[cfg(feature = "redstone")]
pub mod redstone;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub enum OraclePriceId {
    Pyth(PriceIdentifier),
    #[cfg(feature = "redstone")]
    RedStone(crate::oracle::redstone::FeedId),
}

impl From<pyth::PriceIdentifier> for OraclePriceId {
    fn from(value: PriceIdentifier) -> Self {
        Self::Pyth(value)
    }
}

#[cfg(feature = "redstone")]
impl From<crate::oracle::redstone::FeedId> for OraclePriceId {
    fn from(value: crate::oracle::redstone::FeedId) -> Self {
        Self::RedStone(value)
    }
}
