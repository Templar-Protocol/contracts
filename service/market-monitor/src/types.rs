//! Core types.

use near_sdk::AccountId;
use serde::{Deserialize, Serialize};
use templar_common::number::Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertZone {
    Green,  // CR >= MCR × (1 + threshold)
    Yellow, // MCR ≤ CR < MCR × (1 + threshold)
    Red,    // CR < MCR
}

#[derive(Debug, Clone)]
pub struct PositionAlert {
    pub borrower: AccountId,
    pub collateralization_ratio: Decimal,
    pub position_value_usd: Decimal,
    pub zone: AlertZone,
    pub distance_from_mcr_pct: Decimal,
}

#[derive(Debug, Clone)]
pub struct MarketReport {
    pub market: AccountId,
    pub mcr_liquidation: Decimal,
    pub red_positions: Vec<PositionAlert>,
    pub yellow_positions: Vec<PositionAlert>,
}

#[derive(Debug, Clone)]
pub struct DailyReport {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub markets: Vec<MarketReport>,
    pub total_positions: usize,
    pub red_count: usize,
    pub yellow_count: usize,
    pub green_count: usize,
    pub red_value_usd: Decimal,
    pub yellow_value_usd: Decimal,
    pub min_position_size_usd: u64,
    pub displayed_red_count: usize,
    pub displayed_yellow_count: usize,
    pub at_risk_threshold_percent: u16,
    pub ignored_markets_count: usize,
}
