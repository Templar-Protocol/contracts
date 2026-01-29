use templar_common::vault::MarketId;
use templar_vault_kernel::TargetId;

/// Convert executor-facing identifiers into kernel TargetId.
pub trait IntoTargetId {
    fn into_target_id(self) -> TargetId;
}

impl IntoTargetId for MarketId {
    fn into_target_id(self) -> TargetId {
        u32::from(self)
    }
}

impl IntoTargetId for &MarketId {
    fn into_target_id(self) -> TargetId {
        u32::from(*self)
    }
}

impl IntoTargetId for TargetId {
    fn into_target_id(self) -> TargetId {
        self
    }
}

/// Convert kernel TargetId into executor MarketId.
pub trait IntoMarketId {
    fn into_market_id(self) -> MarketId;
}

impl IntoMarketId for TargetId {
    fn into_market_id(self) -> MarketId {
        MarketId::from(self)
    }
}

impl IntoMarketId for &TargetId {
    fn into_market_id(self) -> MarketId {
        MarketId::from(*self)
    }
}
