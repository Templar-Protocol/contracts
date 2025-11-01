use near_sdk::near;

use super::{Contract, U128};

//// AUM (Assets Under Management)
///
/// BalanceSheet model only: total assets are the sum of idle_balance and all market principals.
/// There is no governance-scoped AUM filtering; accounting changes only when cash actually moves.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone)]
pub enum AUM {
    /// BalanceSheet: balance sheet = truth for AUM. See module docs for tradeoffs.
    BalanceSheet,
}

impl AUM {
    /// Compute total assets (BalanceSheet): idle balance + sum of all market principals.
    pub fn get_total_assets(&self, c: &Contract) -> U128 {
        U128(c.markets.iter().fold(c.idle_balance, |prev, (_, rec)| {
            prev.saturating_add(rec.principal)
        }))
    }
}
