//! Scan scheduler.
//!
//! Supports two scheduling modes:
//! - Interval: `*/N` runs every N minutes (e.g., `*/5` for every 5 minutes)
//! - Daily: `HH:MM` runs once per day at the specified UTC time

use chrono::{Datelike, Duration, TimeZone, Utc};
use std::str::FromStr;
use tokio::time::{sleep_until, Instant};

pub struct Scheduler {
    scan_time: String, // HH:MM format or */N for interval in minutes
    first_run: bool,   // Track if this is the first run
}

impl Scheduler {
    pub fn new(scan_time: String) -> Self {
        Self {
            scan_time,
            first_run: true,
        }
    }

    pub async fn wait_until_next_run(&mut self) {
        // For interval-based scheduling, run immediately on first call
        if self.first_run && self.scan_time.starts_with("*/") {
            tracing::info!("First run with interval scheduling - executing immediately");
            self.first_run = false;
            return;
        }

        self.first_run = false;
        let next_run = self.calculate_next_run();
        let now = Utc::now();

        let duration_until_next = next_run.signed_duration_since(now);

        if duration_until_next.num_seconds() <= 0 {
            tracing::warn!("Next run time is in the past, running immediately");
            return;
        }

        tracing::info!(
            next_run = %next_run.format("%Y-%m-%d %H:%M:%S UTC"),
            wait_seconds = duration_until_next.num_seconds(),
            "Waiting for next scheduled run"
        );

        #[allow(clippy::cast_sign_loss)]
        let wait_duration =
            std::time::Duration::from_secs(duration_until_next.num_seconds().max(0) as u64);
        let target_instant = Instant::now() + wait_duration;

        sleep_until(target_instant).await;
    }

    pub fn calculate_next_run(&self) -> chrono::DateTime<Utc> {
        let now = Utc::now();

        // Check if it's an interval format (*/N)
        if self.scan_time.starts_with("*/") {
            let interval_str = self.scan_time.trim_start_matches("*/");
            if let Ok(minutes) = u32::from_str(interval_str) {
                if minutes > 0 {
                    return now + Duration::minutes(i64::from(minutes));
                }
            }
            tracing::warn!("Invalid interval format, defaulting to 5 minutes");
            return now + Duration::minutes(5);
        }

        // Parse HH:MM format
        let parts: Vec<&str> = self.scan_time.split(':').collect();
        let hour = u32::from_str(parts.first().unwrap_or(&"0"))
            .unwrap_or(0)
            .min(23);
        let minute = u32::from_str(parts.get(1).unwrap_or(&"0"))
            .unwrap_or(0)
            .min(59);

        // Today at scan_time
        let today_run = Utc
            .with_ymd_and_hms(now.year(), now.month(), now.day(), hour, minute, 0)
            .unwrap();

        // If today's time has passed, schedule for tomorrow
        if now >= today_run {
            today_run + Duration::days(1)
        } else {
            today_run
        }
    }
}
