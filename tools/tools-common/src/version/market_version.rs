pub struct Market;
pub type MarketVersion = super::Version<Market>;

impl MarketVersion {
    pub fn supports_partial_liquidation(self) -> bool {
        self >= (1, 1, 0)
    }

    pub fn requires_static_yield_accumulation(self) -> bool {
        self >= (1, 1, 0)
    }
}
