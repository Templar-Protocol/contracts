//! Market Monitor - Position health alerting service.
//!
//! Monitors Templar Protocol lending markets on NEAR blockchain for positions
//! at risk of liquidation. Scans positions periodically and sends alerts via
//! Telegram when positions fall below configured health thresholds.

mod analyzer;
mod config;
mod error;
mod processor;
mod reporter;
mod rpc;
mod scanner;
mod scheduler;
mod telegram;
mod types;

use analyzer::Analyzer;
use chrono::Utc;
use config::Config;
use error::Result;
use processor::process_markets;
use reporter::Reporter;
use scanner::MarketScanner;
use scheduler::Scheduler;
use std::time::Instant;
use telegram::TelegramClient;
use types::DailyReport;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_line_number(false)
                .with_file(false),
        )
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    tracing::info!("Starting Templar Market Monitor");

    // Load configuration
    let config = Config::from_env()?;

    tracing::info!(
        network = %config.network,
        rpc_url = %config.rpc_url,
        registries = ?config.registry_account_ids,
        scan_time = %config.scan_time,
        telegram_configured = !config.telegram_bot_token.is_empty(),
        "Configuration loaded"
    );

    // Run continuously on schedule
    tracing::info!("Starting continuous monitoring mode");
    let mut scheduler = Scheduler::new(config.scan_time.clone());

    loop {
        scheduler.wait_until_next_run().await;

        tracing::info!("Starting scheduled scan");
        if let Err(e) = run_scan(&config).await {
            tracing::error!(error = %e, "Scan failed");
        }
    }
}

async fn run_scan(config: &Config) -> Result<()> {
    let start = Instant::now();
    let timestamp = Utc::now();

    tracing::info!(
        "Scan started at {}",
        timestamp.format("%Y-%m-%d %H:%M:%S UTC")
    );

    // Initialize components
    let scanner = MarketScanner::new(&config.rpc_url);
    let analyzer = Analyzer::new(config);

    // Process all markets
    let (market_reports, stats) = process_markets(config, &scanner, &analyzer).await?;

    let scan_duration = start.elapsed();

    // Create daily report
    let displayed_red_count = market_reports.iter().map(|m| m.red_positions.len()).sum();
    let displayed_yellow_count = market_reports
        .iter()
        .map(|m| m.yellow_positions.len())
        .sum();

    let report = DailyReport {
        timestamp,
        markets: market_reports,
        total_positions: stats.total_positions,
        red_count: stats.red_count,
        yellow_count: stats.yellow_count,
        green_count: stats.green_count,
        red_value_usd: stats.red_value_usd,
        yellow_value_usd: stats.yellow_value_usd,
        min_position_size_usd: config.min_position_size_usd,
        displayed_red_count,
        displayed_yellow_count,
        at_risk_threshold_percent: config.at_risk_threshold_percent,
        ignored_markets_count: stats.ignored_markets_count,
    };

    // Format report
    let report_text = Reporter::format_report(&report);

    // Log report preview
    tracing::info!("Report generated, preview:");
    tracing::info!("─────────────────────────────────────");
    for line in report_text.lines() {
        tracing::info!("{}", line);
    }
    tracing::info!("─────────────────────────────────────");
    tracing::info!(
        red_count = report.red_count,
        yellow_count = report.yellow_count,
        total_positions = report.total_positions,
        scan_duration_secs = scan_duration.as_secs(),
        "Scan summary"
    );

    // Send to Telegram (only if token is configured)
    if config.telegram_bot_token.is_empty() {
        tracing::info!("Telegram token not configured - report not sent");
    } else {
        let telegram = TelegramClient::new(config.telegram_bot_token.clone());
        telegram
            .send_message(
                &config.telegram_channel_id,
                &report_text,
                config.telegram_thread_id,
            )
            .await?;
        tracing::info!("Report sent to Telegram successfully");
    }

    Ok(())
}
