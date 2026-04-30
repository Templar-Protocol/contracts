use templar_common::oracle::{pyth::PriceIdentifier, redstone::FeedId};

use templar_gateway_core::OraclePayloadSource;

pub trait ProvidesPythSource {
    type PythSource: OraclePayloadSource<PriceId = PriceIdentifier>;

    fn pyth_source(&self) -> &Self::PythSource;
}

pub trait ProvidesRedStoneSource {
    type RedStoneSource: OraclePayloadSource<PriceId = FeedId>;

    fn redstone_source(&self) -> &Self::RedStoneSource;
}
