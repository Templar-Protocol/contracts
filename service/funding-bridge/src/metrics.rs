//! Prometheus metrics for monitoring service health and performance

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec, CounterVec, Encoder,
    GaugeVec, HistogramVec, TextEncoder,
};

lazy_static! {
    /// HTTP request counter by endpoint and status
    pub static ref HTTP_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "funding_bridge_http_requests_total",
        "Total number of HTTP requests",
        &["endpoint", "method", "status"]
    )
    .unwrap();

    /// HTTP request duration histogram
    pub static ref HTTP_REQUEST_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "funding_bridge_http_request_duration_seconds",
        "HTTP request duration in seconds",
        &["endpoint", "method"],
        vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]
    )
    .unwrap();

    /// Deposit operation counter by status
    pub static ref DEPOSIT_OPERATIONS_TOTAL: CounterVec = register_counter_vec!(
        "funding_bridge_deposit_operations_total",
        "Total number of deposit operations",
        &["status", "chain"]
    )
    .unwrap();

    /// Withdraw operation counter by status
    pub static ref WITHDRAW_OPERATIONS_TOTAL: CounterVec = register_counter_vec!(
        "funding_bridge_withdraw_operations_total",
        "Total number of withdraw operations",
        &["status", "chain"]
    )
    .unwrap();

    /// Chain handler availability gauge
    pub static ref CHAIN_HANDLER_AVAILABLE: GaugeVec = register_gauge_vec!(
        "funding_bridge_chain_handler_available",
        "Chain handler availability (1 = available, 0 = unavailable)",
        &["chain"]
    )
    .unwrap();

    /// Active operations gauge by type
    pub static ref ACTIVE_OPERATIONS: GaugeVec = register_gauge_vec!(
        "funding_bridge_active_operations",
        "Number of active operations",
        &["operation_type", "status"]
    )
    .unwrap();

    /// Treasury balance gauge (requires periodic updates)
    pub static ref TREASURY_BALANCE: GaugeVec = register_gauge_vec!(
        "funding_bridge_treasury_balance",
        "Treasury balance by chain and asset",
        &["chain", "asset"]
    )
    .unwrap();
}

/// Record an HTTP request
pub fn record_http_request(endpoint: &str, method: &str, status: u16, duration_secs: f64) {
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[endpoint, method, &status.to_string()])
        .inc();

    HTTP_REQUEST_DURATION_SECONDS
        .with_label_values(&[endpoint, method])
        .observe(duration_secs);
}

/// Record a deposit operation
pub fn record_deposit(status: &str, chain: &str) {
    DEPOSIT_OPERATIONS_TOTAL
        .with_label_values(&[status, chain])
        .inc();
}

/// Record a withdraw operation
pub fn record_withdraw(status: &str, chain: &str) {
    WITHDRAW_OPERATIONS_TOTAL
        .with_label_values(&[status, chain])
        .inc();
}

/// Update chain handler availability
pub fn set_chain_availability(chain: &str, available: bool) {
    CHAIN_HANDLER_AVAILABLE
        .with_label_values(&[chain])
        .set(if available { 1.0 } else { 0.0 });
}

/// Update treasury balance
pub fn set_treasury_balance(chain: &str, asset: &str, balance: f64) {
    TREASURY_BALANCE
        .with_label_values(&[chain, asset])
        .set(balance);
}

/// Get all metrics in Prometheus text format
pub fn get_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_http_request() {
        record_http_request("/health", "GET", 200, 0.01);
        // Metric should be recorded (no panic)
    }

    #[test]
    fn test_record_deposit() {
        record_deposit("COMPLETED", "near");
        // Metric should be recorded (no panic)
    }

    #[test]
    fn test_set_chain_availability() {
        set_chain_availability("near", true);
        set_chain_availability("ethereum", false);
        // Metrics should be set (no panic)
    }

    #[test]
    fn test_get_metrics() {
        // Record a metric first to ensure output contains our metrics
        record_deposit("COMPLETED", "near");

        let metrics = get_metrics();
        assert!(!metrics.is_empty(), "Metrics output should not be empty");
        assert!(
            metrics.contains("funding_bridge_deposit_operations_total"),
            "Metrics should contain deposit counter"
        );
    }
}
