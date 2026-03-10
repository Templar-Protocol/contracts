use super::*;

/// Vault operation errors.
#[derive(Debug)]
#[near(serializers = [json])]
pub enum Error {
    /// Index drift or stale op_id.
    IndexDrifted(ExpectedIdx, ActualIdx),
    /// Callback resolved different market.
    MarketDrifted {
        expected: MarketId,
        actual: MarketId,
    },
    /// Unknown market.
    MissingMarket(MarketId),
    /// Not in withdrawing state.
    NotWithdrawing,
    /// Not in allocating state.
    NotAllocating,
    /// Not in refreshing state.
    NotRefreshing,
    /// Not in payout state.
    NotPayout,
    /// Market transfer failed.
    MarketTransferFailed,
    /// Supply position not found.
    MissingSupplyPosition,
    /// Position read failed.
    PositionReadFailed,
    /// Balance read failed.
    BalanceReadFailed,
    /// Insufficient liquidity across markets.
    InsufficientLiquidity,
    /// Zero amount provided.
    ZeroAmount,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use crate::vault::MarketId;

    #[test]
    fn display_uses_debug_shape() {
        let error = Error::MarketDrifted {
            expected: MarketId(1),
            actual: MarketId(2),
        };

        assert_eq!(
            error.to_string(),
            "MarketDrifted { expected: MarketId(1), actual: MarketId(2) }"
        );
    }
}
