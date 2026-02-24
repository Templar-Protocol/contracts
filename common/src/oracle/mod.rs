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
    RedStone(String),
}
