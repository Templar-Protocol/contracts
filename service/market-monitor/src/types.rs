//! Core types.

use near_sdk::AccountId;
use serde::{Deserialize, Serialize};
use templar_common::Decimal;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_zone_classification() {
        // Test that alert zones are properly classified
        let red = AlertZone::Red;
        let yellow = AlertZone::Yellow;
        let green = AlertZone::Green;

        assert_ne!(red, yellow);
        assert_ne!(red, green);
        assert_ne!(yellow, green);
    }

    #[test]
    fn test_position_alert_creation() {
        let alert = PositionAlert {
            borrower: "test.near".parse().unwrap(),
            collateralization_ratio: Decimal::from(120u32),
            position_value_usd: Decimal::from(5000u32),
            zone: AlertZone::Yellow,
            distance_from_mcr_pct: Decimal::from(10u32),
        };

        assert_eq!(alert.zone, AlertZone::Yellow);
        assert_eq!(alert.collateralization_ratio, Decimal::from(120u32));
        assert_eq!(alert.position_value_usd, Decimal::from(5000u32));
    }

    #[test]
    fn test_daily_report_statistics() {
        let timestamp = chrono::Utc::now();

        let report = DailyReport {
            timestamp,
            markets: vec![],
            total_positions: 100,
            red_count: 5,
            yellow_count: 15,
            green_count: 80,
            red_value_usd: Decimal::from(100_000_u32),
            yellow_value_usd: Decimal::from(250_000_u32),
            min_position_size_usd: 1000,
            displayed_red_count: 3,
            displayed_yellow_count: 10,
            at_risk_threshold_percent: 10,
            ignored_markets_count: 2,
        };

        // Verify counts add up
        assert_eq!(
            report.total_positions,
            report.red_count + report.yellow_count + report.green_count
        );

        // Verify display counts are less than or equal to total counts
        assert!(report.displayed_red_count <= report.red_count);
        assert!(report.displayed_yellow_count <= report.yellow_count);
    }

    #[test]
    fn test_ignored_markets_tracking() {
        let timestamp = chrono::Utc::now();

        let report = DailyReport {
            timestamp,
            markets: vec![],
            total_positions: 50,
            red_count: 0,
            yellow_count: 0,
            green_count: 50,
            red_value_usd: Decimal::ZERO,
            yellow_value_usd: Decimal::ZERO,
            min_position_size_usd: 1000,
            displayed_red_count: 0,
            displayed_yellow_count: 0,
            at_risk_threshold_percent: 10,
            ignored_markets_count: 5,
        };

        // Verify the structure contains ignored markets count
        assert_eq!(report.ignored_markets_count, 5);
    }
}
