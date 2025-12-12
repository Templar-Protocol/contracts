use chrono::Timelike;
use templar_common::number::Decimal;
use templar_market_monitor::{
    reporter::Reporter,
    scheduler::Scheduler,
    types::{AlertZone, DailyReport, MarketReport, PositionAlert},
};

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
fn test_scheduler_interval_parsing() {
    let scheduler = Scheduler::new("*/5".to_string());
    let next_run = scheduler.calculate_next_run();
    let now = chrono::Utc::now();

    // Next run should be within the next 5 minutes
    let duration = next_run.signed_duration_since(now);
    assert!(duration.num_seconds() > 0);
    assert!(duration.num_seconds() <= 5 * 60);
}

#[test]
fn test_scheduler_daily_parsing() {
    let scheduler = Scheduler::new("14:30".to_string());
    let next_run = scheduler.calculate_next_run();
    let now = chrono::Utc::now();

    // Next run should be in the future
    let duration = next_run.signed_duration_since(now);
    assert!(duration.num_seconds() > 0);

    // Next run should be within 24 hours
    assert!(duration.num_hours() <= 24);

    // Hour and minute should match
    assert_eq!(next_run.hour(), 14);
    assert_eq!(next_run.minute(), 30);
}

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

    let formatted = Reporter::format_report(&report);

    // Verify ignored markets are shown in the report
    assert!(formatted.contains("ignored"));
}
