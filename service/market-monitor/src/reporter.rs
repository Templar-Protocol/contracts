//! Report formatter.
//!
//! Generates human-readable Telegram reports with:
//! - Summary statistics (market counts, position distributions)
//! - Liquidatable positions (urgent action required)
//! - At-risk positions (approaching liquidation)
//! - Position details including CR, debt value, and distance from MCR

use crate::types::{DailyReport, MarketReport, PositionAlert};
use std::fmt::Write;
use templar_common::Decimal;

pub struct Reporter;

impl Reporter {
    #[allow(clippy::too_many_lines)]
    pub fn format_report(report: &DailyReport) -> String {
        let mut output = String::new();

        // Header
        output.push_str("📊 TEMPLAR MARKETS REPORT\n");
        writeln!(
            output,
            "Date: {}",
            report.timestamp.format("%Y-%m-%d %H:%M UTC")
        )
        .unwrap();
        writeln!(
            output,
            "At Risk Threshold: {}% above MCR | Min Position Display Size: ${}",
            report.at_risk_threshold_percent, report.min_position_size_usd
        )
        .unwrap();
        output.push('\n');

        // Count positions by zone
        let has_red = report.red_count > 0;
        let has_yellow = report.yellow_count > 0;

        // Summary first
        output.push_str("📈 SUMMARY\n");
        let total_discovered = report.markets.len() + report.ignored_markets_count;
        if report.ignored_markets_count > 0 {
            writeln!(
                output,
                "Markets: {} active, {} ignored ({} total)",
                report.markets.len(),
                report.ignored_markets_count,
                total_discovered
            )
            .unwrap();
        } else {
            writeln!(output, "Markets: {}", report.markets.len()).unwrap();
        }
        writeln!(output, "Total Positions: {}", report.total_positions).unwrap();
        #[allow(clippy::cast_precision_loss)]
        {
            writeln!(
                output,
                "  ├─ 🟢 Healthy: {} ({:.1}%)",
                report.green_count,
                (report.green_count as f64 / report.total_positions as f64) * 100.0
            )
            .unwrap();
            writeln!(
                output,
                "  ├─ 🟡 At Risk: {} ({:.1}%)",
                report.yellow_count,
                (report.yellow_count as f64 / report.total_positions as f64) * 100.0
            )
            .unwrap();
            writeln!(
                output,
                "  └─ 🔴 Liquidatable: {} ({:.1}%)",
                report.red_count,
                (report.red_count as f64 / report.total_positions as f64) * 100.0
            )
            .unwrap();
        }

        writeln!(
            output,
            "\nAt Risk Value: ${}",
            Self::format_usd(report.yellow_value_usd)
        )
        .unwrap();

        writeln!(
            output,
            "Liquidatable Value: ${}\n",
            Self::format_usd(report.red_value_usd)
        )
        .unwrap();

        // Liquidatable section
        if has_red {
            writeln!(output, "🔴 LIQUIDATABLE ({} position(s))", report.red_count).unwrap();
            output.push_str("Positions below liquidation MCR\n");
            if report.displayed_red_count < report.red_count {
                writeln!(
                    output,
                    "Displaying {} positions > ${} USD\n",
                    report.displayed_red_count, report.min_position_size_usd
                )
                .unwrap();
            } else {
                output.push('\n');
            }

            for market_report in &report.markets {
                if !market_report.red_positions.is_empty() {
                    Self::format_market_section(&mut output, market_report, true);
                }
            }
        }

        // At risk section
        if has_yellow {
            writeln!(output, "🟡 AT RISK ({} position(s))", report.yellow_count).unwrap();
            output.push_str("Positions approaching liquidation\n");
            if report.displayed_yellow_count < report.yellow_count {
                writeln!(
                    output,
                    "Displaying {} positions > ${} USD\n",
                    report.displayed_yellow_count, report.min_position_size_usd
                )
                .unwrap();
            } else {
                output.push('\n');
            }

            for market_report in &report.markets {
                if !market_report.yellow_positions.is_empty() {
                    Self::format_market_section(&mut output, market_report, false);
                }
            }
        }

        // If no alerts, add note
        if !has_red && !has_yellow {
            output.push_str("\n✅ ALL POSITIONS HEALTHY\n");
            output.push_str("No positions require attention\n");
        }

        output
    }

    fn format_market_section(output: &mut String, market_report: &MarketReport, is_red: bool) {
        writeln!(output, "\nMarket: {}", market_report.market).unwrap();

        #[allow(clippy::cast_precision_loss)]
        let mcr_f64: f64 = market_report
            .mcr_liquidation
            .to_string()
            .parse()
            .unwrap_or(0.0);

        writeln!(
            output,
            "MCR Liquidation: {:.2} ({:.2}%)\n",
            mcr_f64,
            mcr_f64 * 100.0
        )
        .unwrap();

        let positions = if is_red {
            &market_report.red_positions
        } else {
            &market_report.yellow_positions
        };

        for (i, position) in positions.iter().enumerate() {
            if i > 0 {
                output.push('\n');
            }
            Self::format_position(output, position);
        }

        output.push('\n');
    }

    fn format_position(output: &mut String, position: &PositionAlert) {
        use crate::types::AlertZone;

        // Red zone = below MCR, Yellow zone = above MCR
        let is_below_mcr = position.zone == AlertZone::Red;
        let direction = if is_below_mcr { "↓" } else { "↑" };

        // Convert Decimal to f64 for formatting
        #[allow(clippy::cast_precision_loss)]
        let cr_f64: f64 = position
            .collateralization_ratio
            .to_string()
            .parse()
            .unwrap_or(0.0);
        #[allow(clippy::cast_precision_loss)]
        let distance_f64: f64 = position
            .distance_from_mcr_pct
            .to_string()
            .parse()
            .unwrap_or(0.0);

        writeln!(output, "  {}", position.borrower).unwrap();
        writeln!(
            output,
            "  CR: {:.2} ({:.2}%) {} {:.2}% {} MCR",
            cr_f64,
            cr_f64 * 100.0,
            direction,
            distance_f64,
            if is_below_mcr { "below" } else { "above" }
        )
        .unwrap();
        // position_value_usd already contains the debt amount adjusted for decimals
        writeln!(
            output,
            "  Debt: ${}",
            Self::format_amount(position.position_value_usd)
        )
        .unwrap();
    }

    fn format_amount(amount: Decimal) -> String {
        #[allow(clippy::cast_precision_loss)]
        let amount_f64: f64 = amount.to_string().parse().unwrap_or(0.0);
        if amount_f64 >= 1_000_000.0 {
            format!("{:.2}M", amount_f64 / 1_000_000.0)
        } else if amount_f64 >= 1_000.0 {
            format!("{:.2}K", amount_f64 / 1_000.0)
        } else {
            format!("{amount_f64:.2}")
        }
    }

    fn format_usd(amount: Decimal) -> String {
        #[allow(clippy::cast_precision_loss)]
        let amount_f64: f64 = amount.to_string().parse().unwrap_or(0.0);
        if amount_f64 >= 1_000_000.0 {
            format!("{:.2}M", amount_f64 / 1_000_000.0)
        } else if amount_f64 >= 1_000.0 {
            format!("{:.2}K", amount_f64 / 1_000.0)
        } else {
            format!("{amount_f64:.2}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AlertZone;

    #[test]
    fn test_report_format_with_alerts() {
        let timestamp = chrono::Utc::now();
        let market_id = "test-market.near".parse().unwrap();

        let red_alert = PositionAlert {
            borrower: "alice.near".parse().unwrap(),
            collateralization_ratio: Decimal::from(105u32),
            position_value_usd: Decimal::from(50000u32),
            zone: AlertZone::Red,
            distance_from_mcr_pct: Decimal::ZERO, // Simplified for testing
        };

        let yellow_alert = PositionAlert {
            borrower: "bob.near".parse().unwrap(),
            collateralization_ratio: Decimal::from(115u32),
            position_value_usd: Decimal::from(30000u32),
            zone: AlertZone::Yellow,
            distance_from_mcr_pct: Decimal::from(5u32),
        };

        let market_report = MarketReport {
            market: market_id,
            mcr_liquidation: Decimal::from(110u32),
            red_positions: vec![red_alert],
            yellow_positions: vec![yellow_alert],
        };

        let report = DailyReport {
            timestamp,
            markets: vec![market_report],
            total_positions: 10,
            red_count: 1,
            yellow_count: 1,
            green_count: 8,
            red_value_usd: Decimal::from(50000u32),
            yellow_value_usd: Decimal::from(30000u32),
            min_position_size_usd: 1000,
            displayed_red_count: 1,
            displayed_yellow_count: 1,
            at_risk_threshold_percent: 10,
            ignored_markets_count: 2,
        };

        let formatted = Reporter::format_report(&report);

        // Verify key sections are present
        assert!(formatted.contains("📊 TEMPLAR MARKETS REPORT"));
        assert!(formatted.contains("📈 SUMMARY"));
        assert!(formatted.contains("🔴 LIQUIDATABLE"));
        assert!(formatted.contains("🟡 AT RISK"));
        assert!(formatted.contains("alice.near"));
        assert!(formatted.contains("bob.near"));
        assert!(formatted.contains("test-market.near"));
    }

    #[test]
    fn test_report_format_all_healthy() {
        let timestamp = chrono::Utc::now();

        let report = DailyReport {
            timestamp,
            markets: vec![],
            total_positions: 25,
            red_count: 0,
            yellow_count: 0,
            green_count: 25,
            red_value_usd: Decimal::ZERO,
            yellow_value_usd: Decimal::ZERO,
            min_position_size_usd: 1000,
            displayed_red_count: 0,
            displayed_yellow_count: 0,
            at_risk_threshold_percent: 10,
            ignored_markets_count: 0,
        };

        let formatted = Reporter::format_report(&report);

        // Verify healthy message is present
        assert!(formatted.contains("ALL POSITIONS HEALTHY"));
        assert!(!formatted.contains("🔴 LIQUIDATABLE"));
        assert!(!formatted.contains("🟡 AT RISK"));
    }

    #[test]
    fn test_format_amount_small() {
        let amount = Decimal::from(100u32);
        let formatted = Reporter::format_amount(amount);
        assert_eq!(formatted, "100.00");
    }

    #[test]
    fn test_format_amount_thousands() {
        let amount = Decimal::from(5000u32);
        let formatted = Reporter::format_amount(amount);
        assert_eq!(formatted, "5.00K");
    }

    #[test]
    fn test_format_amount_millions() {
        let amount = Decimal::from(2_500_000u32);
        let formatted = Reporter::format_amount(amount);
        assert_eq!(formatted, "2.50M");
    }

    #[test]
    fn test_format_usd_small() {
        let amount = Decimal::from(50u32);
        let formatted = Reporter::format_usd(amount);
        assert_eq!(formatted, "50.00");
    }

    #[test]
    fn test_format_usd_thousands() {
        let amount = Decimal::from(12_500u32);
        let formatted = Reporter::format_usd(amount);
        assert_eq!(formatted, "12.50K");
    }

    #[test]
    fn test_format_usd_millions() {
        let amount = Decimal::from(1_000_000u32);
        let formatted = Reporter::format_usd(amount);
        assert_eq!(formatted, "1.00M");
    }

    #[test]
    fn test_report_with_only_red_positions() {
        let timestamp = chrono::Utc::now();
        let market_id = "test-market.near".parse().unwrap();

        let red_alert1 = PositionAlert {
            borrower: "user1.near".parse().unwrap(),
            collateralization_ratio: Decimal::from(105u32),
            position_value_usd: Decimal::from(10000u32),
            zone: AlertZone::Red,
            distance_from_mcr_pct: Decimal::ZERO,
        };

        let red_alert2 = PositionAlert {
            borrower: "user2.near".parse().unwrap(),
            collateralization_ratio: Decimal::from(108u32),
            position_value_usd: Decimal::from(5000u32),
            zone: AlertZone::Red,
            distance_from_mcr_pct: Decimal::ZERO,
        };

        let market_report = MarketReport {
            market: market_id,
            mcr_liquidation: Decimal::from(110u32),
            red_positions: vec![red_alert1, red_alert2],
            yellow_positions: vec![],
        };

        let report = DailyReport {
            timestamp,
            markets: vec![market_report],
            total_positions: 5,
            red_count: 2,
            yellow_count: 0,
            green_count: 3,
            red_value_usd: Decimal::from(15000u32),
            yellow_value_usd: Decimal::ZERO,
            min_position_size_usd: 1000,
            displayed_red_count: 2,
            displayed_yellow_count: 0,
            at_risk_threshold_percent: 10,
            ignored_markets_count: 0,
        };

        let formatted = Reporter::format_report(&report);
        assert!(formatted.contains("🔴 LIQUIDATABLE"));
        assert!(formatted.contains("user1.near"));
        assert!(formatted.contains("user2.near"));
        assert!(!formatted.contains("🟡 AT RISK"));
    }

    #[test]
    fn test_report_with_only_yellow_positions() {
        let timestamp = chrono::Utc::now();
        let market_id = "test-market.near".parse().unwrap();

        let yellow_alert = PositionAlert {
            borrower: "user.near".parse().unwrap(),
            collateralization_ratio: Decimal::from(115u32),
            position_value_usd: Decimal::from(8000u32),
            zone: AlertZone::Yellow,
            distance_from_mcr_pct: Decimal::from(5u32),
        };

        let market_report = MarketReport {
            market: market_id,
            mcr_liquidation: Decimal::from(110u32),
            red_positions: vec![],
            yellow_positions: vec![yellow_alert],
        };

        let report = DailyReport {
            timestamp,
            markets: vec![market_report],
            total_positions: 3,
            red_count: 0,
            yellow_count: 1,
            green_count: 2,
            red_value_usd: Decimal::ZERO,
            yellow_value_usd: Decimal::from(8000u32),
            min_position_size_usd: 1000,
            displayed_red_count: 0,
            displayed_yellow_count: 1,
            at_risk_threshold_percent: 10,
            ignored_markets_count: 0,
        };

        let formatted = Reporter::format_report(&report);
        assert!(!formatted.contains("🔴 LIQUIDATABLE"));
        assert!(formatted.contains("🟡 AT RISK"));
        assert!(formatted.contains("user.near"));
    }
}
