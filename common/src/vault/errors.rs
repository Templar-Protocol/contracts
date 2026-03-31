use super::*;

/// Vault operation errors.
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

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::IndexDrifted(expected, actual) => {
                write!(f, "vault index drifted: expected {expected}, got {actual}")
            }
            Error::MarketDrifted { expected, actual } => {
                write!(f, "vault market drifted: expected {expected}, got {actual}")
            }
            Error::MissingMarket(market) => write!(f, "missing market: {market}"),
            Error::NotWithdrawing => f.write_str("vault is not withdrawing"),
            Error::NotAllocating => f.write_str("vault is not allocating"),
            Error::NotRefreshing => f.write_str("vault is not refreshing"),
            Error::NotPayout => f.write_str("vault is not in payout"),
            Error::MarketTransferFailed => f.write_str("market transfer failed"),
            Error::MissingSupplyPosition => f.write_str("missing supply position"),
            Error::PositionReadFailed => f.write_str("position read failed"),
            Error::BalanceReadFailed => f.write_str("balance read failed"),
            Error::InsufficientLiquidity => f.write_str("insufficient liquidity"),
            Error::ZeroAmount => f.write_str("zero amount"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use crate::vault::MarketId;

    #[test]
    fn display_uses_human_readable_message() {
        let error = Error::MarketDrifted {
            expected: MarketId(1),
            actual: MarketId(2),
        };

        assert_eq!(error.to_string(), "vault market drifted: expected 1, got 2");
    }
}
